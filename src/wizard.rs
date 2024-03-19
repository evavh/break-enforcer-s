use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use color_eyre::eyre::Context;
use color_eyre::Result;
use dialoguer::{Confirm, MultiSelect};
use itertools::Itertools;

use crate::config::{self, InputFilter};
use crate::watch::{BlockableInput, OnlineDevices};

// todo deal with devices with multiple names
pub fn run(devices: &OnlineDevices, custom_config_path: Option<PathBuf>) -> Result<()> {
    let config: HashMap<_, _> = config::read(custom_config_path.clone())
        .wrap_err("Could not read custom config")?
        .into_iter()
        .map(|InputFilter { id, names }| (id, names))
        .collect();

    let mut inputs = devices.list_inputs();
    for BlockableInput { names, .. } in &mut inputs {
        names.sort()
    }
    let mut inputs: Vec<_> = inputs
        .into_iter()
        .flat_map(|BlockableInput { names, id }| names.into_iter().map(move |n| (id, n)))
        .collect();
    inputs.dedup_by(|a, b| *a == *b);

    let mut options: Vec<_> = inputs
        .iter()
        .map(|(id, name)| {
            let checked = config
                .get(id)
                .map(|names| names.contains(name))
                .unwrap_or(false);
            (name, checked)
        })
        .collect();

    loop {
        let Some(selection) = MultiSelect::new()
            .with_prompt("Use up and down arrow keys and space to select. Enter to continue")
            .items_checked(&options[..])
            .interact_opt()
            .unwrap()
        else {
            println!("No devices selected");
            return Ok(());
        };

        {
            println!("Locking devices, do not press any key!");
            // do not lock while user is still holding down
            // enter from the multiselect
            thread::sleep(Duration::from_secs(2));
            for option in &mut options {
                option.1 = false;
            }
            for idx in &selection {
                options[*idx].1 = true
            }

            let locked: Vec<_> = selection
                .iter()
                .map(|checked| inputs[*checked].clone())
                .into_group_map()
                .into_iter()
                .map(|(id, names)| InputFilter {
                    id,
                    names: names.clone(),
                })
                .map(|filter| devices.lock(filter))
                .collect::<Result<_>>()?;

            println!("Try to use them, they should be blocked");
            thread::sleep(Duration::from_secs(8));
            println!("\n\nUnlocking, Stop typing!");
            for lock in locked {
                lock.unlock()?
            }
        }
        thread::sleep(Duration::from_secs(2));

        let Some(ready) = Confirm::new()
            .with_prompt("Are you happy with the blocked devices?")
            .interact_opt()
            .unwrap()
        else {
            println!("Cancelling");
            return Ok(());
        };

        if ready {
            let selected: Vec<InputFilter> = inputs
                .into_iter()
                .enumerate()
                .filter(|(i, _)| selection.contains(i))
                .map(|(_, (id, name))| (id, name))
                .into_group_map()
                .into_iter()
                .map(|(id, names)| InputFilter { id, names })
                .collect();
            config::write(&selected, custom_config_path).unwrap();
            return Ok(());
        }
    }
}
