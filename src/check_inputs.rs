use std::{
    fs::{self, File},
    io::{self, Read},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    thread,
};

use crate::{
    config::InputFilter,
    watch::NewInput,
};

pub fn wait_for_input(file: &mut File) -> std::io::Result<()> {
    let mut packet = [0u8; 24];
    file.read_exact(&mut packet)
}

pub fn inactivity_watcher(
    work_start_receiver: &Receiver<bool>,
    break_skip_sender: &Sender<bool>,
    break_skip_sent: &Arc<AtomicBool>,
    input_receiver: &Receiver<InputResult>,
    work_duration: std::time::Duration,
) {
    work_start_receiver.recv().unwrap();

    loop {
        match input_receiver.recv_timeout(work_duration) {
            Ok(Ok(_)) => (),
            Ok(Err(_)) => return,
            Err(RecvTimeoutError::Timeout) => {
                if !break_skip_sent.load(Ordering::Acquire) {
                    break_skip_sender.send(true).unwrap();
                    break_skip_sent.store(true, Ordering::Release);
                }
            }
            Err(e) => panic!("Unexpected error: {e}"),
        }
    }
}

pub type InputResult = Result<bool, Arc<io::Error>>;

pub(crate) fn watcher(
    new: Receiver<NewInput>,
    to_block: Vec<InputFilter>,
) -> color_eyre::Result<(
    Receiver<Result<bool, Arc<io::Error>>>,
    Receiver<Result<bool, Arc<io::Error>>>,
)> {
    let (tx1, rx1) = channel();
    let (tx2, rx2) = channel();

    thread::spawn(move || loop {
        let input = new.recv().unwrap();
        if !to_block
            .iter()
            .filter(|filter| filter.id == input.id)
            .any(|filter| filter.names.contains(&input.name))
        {
            continue;
        }

        let tx1 = tx1.clone();
        let tx2 = tx2.clone();
        thread::Builder::new()
            .spawn(move || loop {
                let mut file = match fs::File::open(&input.path) {
                    // must be closed
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                    Err(e) => {
                        // unexpected error, report to main thread
                        let err = Arc::new(e); // make cloneable
                        let _ig_err = tx1.send(Err(err.clone()));
                        let _ig_err = tx2.send(Err(err));
                        return;
                    }
                    Ok(file) => file,
                };

                match wait_for_input(&mut file) {
                    // must be closed
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                    Err(e) => {
                        // unexpected error, report to main thread
                        let err = Arc::new(e); // make cloneable
                        let _ig_err = tx1.send(Err(err.clone()));
                        let _ig_err = tx2.send(Err(err));
                        return;
                    }
                    Ok(_) => (),
                };

                let _ = tx1.send(Ok(true));
                let _ = tx2.send(Ok(true));
            })
            .unwrap();
    });

    Ok((rx1, rx2))
}
