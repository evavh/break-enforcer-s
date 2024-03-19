use core::fmt;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::{engine::general_purpose, Engine as _};
use color_eyre::eyre::Context;
use color_eyre::{Result, Section};
use serde::{Deserialize, Serialize};

use crate::config::InputFilter;

struct Device {
    raw_dev: evdev::Device,
}

/// need to modify Device (grap/ungrap) therefore can not use a Set
/// instead we use a Map with a characteristic of the device as
/// "Key"
// File descriptor is unique within a process so a good key
#[derive(Hash, PartialEq, Eq)]
struct DeviceKey(std::os::fd::RawFd);

fn device_name(device: &evdev::Device) -> String {
    device
        .name()
        .or(device.unique_name())
        .map(String::from)
        .unwrap_or_else(|| {
            let id = InputId::from(device.input_id());
            format!("Unknown device, id: {}", id)
        })
}

impl Device {
    fn name(&self) -> String {
        device_name(&self.raw_dev)
    }

    fn key(&self) -> DeviceKey {
        DeviceKey(self.raw_dev.as_raw_fd())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct InputId {
    vendor: u16,
    product: u16,
    version: u16,
}

impl fmt::Display for InputId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = [
            self.vendor.to_be_bytes(),
            self.product.to_be_bytes(),
            self.version.to_be_bytes(),
        ];

        let base64 = general_purpose::URL_SAFE_NO_PAD.encode(data.flatten());
        f.write_str(base64.as_str())
    }
}

impl From<evdev::InputId> for InputId {
    fn from(value: evdev::InputId) -> Self {
        Self {
            vendor: value.vendor(),
            product: value.product(),
            version: value.version(),
        }
    }
}

macro_rules! lock_and_call_inner {
    ($is_pub:vis $name:ident, $($arg:ident: $type:ty),* $(;$ret:ty)?) => {
        $is_pub fn $name(&self, $($arg: $type),*) $(-> $ret)? {
            self.inner.lock().unwrap().$name($($arg),*)
        }
    };
}

#[derive(Clone)]
pub struct OnlineDevices {
    tx: mpsc::Sender<Order>,
    inner: Arc<Mutex<Inner>>,
}

impl OnlineDevices {
    lock_and_call_inner!(pub list_inputs,; Vec<BlockableInput>);
    lock_and_call_inner!(insert, raw_dev: evdev::Device; bool);
    lock_and_call_inner!(lock_all_matching, id: InputFilter; Result<()>);
    lock_and_call_inner!(unlock_all_matching, id: InputFilter; Result<()>);

    /// will also ensure that if the device is connected before
    /// the lockguard is dropped that it is locked
    pub(crate) fn lock(&self, input: InputFilter) -> Result<LockGuard> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.tx
            .send(Order::Lock(input.clone(), tx))
            .expect("devices should never end/panic");

        let lock_res = rx.recv().expect("devices should never end/panic");
        lock_res.wrap_err("Could not lock device")?;

        Ok(LockGuard {
            filter: input,
            tx: self.tx.clone(),
            dropped: false,
        })
    }
}

enum Order {
    Lock(InputFilter, mpsc::Sender<Result<()>>),
    UnLock(InputFilter, mpsc::Sender<Result<()>>),
}

/// use `unlock` to re-enable the disabled input device
#[must_use]
pub struct LockGuard {
    filter: InputFilter,
    tx: mpsc::Sender<Order>,
    // skip backup unlock if user did things right
    dropped: bool,
}

impl LockGuard {
    pub(crate) fn unlock(mut self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.tx
            .send(Order::UnLock(self.filter.clone(), tx))
            .expect("devices should never end/panic");

        rx.recv().expect("devices should never end/panic")?;
        self.dropped = true;
        Ok(())
    }
}

/// backup, user should call unlock!
impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.dropped {
            return; // nothing to do
        }
        let (tx, _) = std::sync::mpsc::channel();
        let _do_not_panic_in_drop = self.tx.send(Order::UnLock(self.filter.clone(), tx));
        eprintln!(
            "Should not drop LockGuard but instead destroy by calling unlock
            since drop can not return an error"
        )
    }
}

struct Inner {
    // multiple devices with the same id could have different
    // names due to manufacturer mistake
    // device serial could be duplicate due to manufacturer mistake
    id_to_devices: HashMap<InputId, HashMap<DeviceKey, Device>>,
}

impl Inner {
    /// if it was already present ignore
    fn insert(&mut self, raw_dev: evdev::Device) -> bool {
        let id = raw_dev.input_id().into();
        let device = Device { raw_dev };
        if let Some(in_map) = self.id_to_devices.get_mut(&id) {
            in_map.insert(device.key(), device).is_some()
        } else {
            self.id_to_devices
                .insert(id, HashMap::from([(device.key(), device)]));
            true
        }
    }

    fn list_inputs(&mut self) -> Vec<BlockableInput> {
        self.id_to_devices
            .iter()
            .map(|(id, devices)| {
                let mut names: Vec<_> = devices.values().map(Device::name).collect();
                names.sort();
                BlockableInput { id: *id, names }
            })
            .collect()
    }

    fn unlock_all_matching(&mut self, filter: InputFilter) -> Result<()> {
        let Some(to_lock) = self.id_to_devices.get_mut(&filter.id) else {
            return Ok(());
        };

        for device in to_lock
            .values_mut()
            .filter(|device| filter.names.contains(&device.name()))
        {
            device
                .raw_dev
                .ungrab()
                .wrap_err("Could not ungrab (release exclusive access) to device")
                .with_note(|| format!("device name: {}", device.name()))?;
        }
        Ok(())
    }

    fn lock_all_matching(&mut self, filter: InputFilter) -> Result<()> {
        let Some(to_lock) = self.id_to_devices.get_mut(&filter.id) else {
            return Ok(());
        };

        for device in to_lock
            .values_mut()
            .filter(|device| filter.names.contains(&device.name()))
        {
            match device.raw_dev.grab() {
                Ok(_) => (),
                Err(e) if e.kind() == ErrorKind::ResourceBusy => (),
                err @ Err(_) => {
                    return err
                        .wrap_err("Could not grab (acquire exclusive access) to device")
                        .with_note(|| format!("device name: {}", device.name()))
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct BlockableInput {
    pub id: InputId,
    pub names: Vec<String>,
}

#[derive(Clone)]
pub struct NewInput {
    pub id: InputId,
    pub name: String,
    pub path: PathBuf,
}

/* TODO: use ionotify instead? (watching /dev/input)
 * would need async to also wait for rx_recv
 * or another thread sending `Order::scan`
 * look at cpu usage first though
 * <15-03-24, dvdsk> */
pub fn devices() -> (OnlineDevices, Receiver<NewInput>) {
    let (order_tx, order_rx) = mpsc::channel();
    let online = OnlineDevices {
        tx: order_tx,
        inner: Arc::new(Mutex::new(Inner {
            id_to_devices: HashMap::new(),
        })),
    };

    let (new_dev_tx, new_dev_rx) = mpsc::channel();
    scan_and_process_new(&online, &new_dev_tx);
    {
        let online = online.clone();
        thread::spawn(move || loop {
            scan_and_process_new(&online, &new_dev_tx);
            match order_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Order::Lock(filter, answer)) => {
                    let res = online.lock_all_matching(filter);
                    answer.send(res).expect("lock fn does not panic");
                }
                Ok(Order::UnLock(filter, answer)) => {
                    let res = online.unlock_all_matching(filter);
                    answer.send(res).expect("unlock fn does not panic");
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        });
    }

    (online, new_dev_rx)
}

fn scan_and_process_new(online: &OnlineDevices, new_dev_tx: &mpsc::Sender<NewInput>) {
    for (event_path, device) in evdev::enumerate() {
        let id = InputId::from(device.input_id());
        let name = device_name(&device);
        let new = online.insert(device);
        if new {
            new_dev_tx
                .send(NewInput {
                    id,
                    name,
                    path: event_path,
                })
                .expect("watcher should never end and drop rx");
        }
    }
}
