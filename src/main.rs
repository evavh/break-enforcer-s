#![feature(thread_sleep_until)]
#![feature(iter_intersperse)]
#![feature(slice_flatten)]
#![feature(io_error_more)]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError},
        Arc,
    },
    thread,
};

use check_inputs::InputResult;
use clap::Parser;
use color_eyre::eyre::Context;

mod check_inputs;
mod cli;
mod config;
mod notification;
mod watch;
mod wizard;

use crate::check_inputs::inactivity_watcher;
use crate::notification::notify_all_users;

fn main() -> color_eyre::Result<()> {
    color_eyre::install().expect("Only called once");
    let args = cli::Cli::parse();

    // check after args such that help can run without root
    if let sudo::RunningAs::User = sudo::check() {
        panic!(concat!(
            "must run ",
            env!("CARGO_CRATE_NAME"),
            " as root user"
        ));
    }

    let (online_devices, new) = watch::devices();
    let (work_duration, break_duration, grace_duration) = match args.command {
        cli::Commands::Run {
            work_duration,
            break_duration,
            grace_duration,
        } => (work_duration, break_duration, grace_duration),
        cli::Commands::Wizard => {
            wizard::run(&online_devices, args.config_path).wrap_err("Error running wizard")?;
            return Ok(());
        }
    };

    let to_block =
        config::read(args.config_path).wrap_err("Could not read devices to block from config")?;
    let (recv_any_input, recv_any_input2) = check_inputs::watcher(new, to_block.clone())
        .wrap_err("Could not start watching to be locked devices for activaty")?;

    let (break_skip_sender, break_skip_receiver) = channel();
    let (work_start_sender, work_start_receiver) = channel();
    let break_skip_is_sent = Arc::new(AtomicBool::new(false));

    {
        let break_skip_is_sent = break_skip_is_sent.clone();

        thread::spawn(move || {
            inactivity_watcher(
                &work_start_receiver,
                &break_skip_sender,
                &break_skip_is_sent,
                &recv_any_input2,
                work_duration,
            );
        });
    }

    loop {
        notify_all_users("Waiting for input to start work timer...");
        block_on_new_input(&recv_any_input).wrap_err("Could not block till new input")?;
        notify_all_users(&format!("Starting work timer for {break_duration:?}"));
        work_start_sender.send(true).unwrap();
        match break_skip_receiver.recv_timeout(break_duration - grace_duration) {
            Ok(_) => {
                notify_all_users("No input for breaktime");
                block_on_new_input(&recv_any_input).wrap_err("Could not block till new input")?;
                break_skip_is_sent.store(false, Ordering::Release);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => (),
            Err(e) => panic!("Unexpected error: {e}"),
        }

        notify_all_users(&format!("Locking in {grace_duration:?}!"));
        thread::sleep(grace_duration);

        let mut locks = Vec::new();

        for device_id in to_block.iter().cloned() {
            locks.push(
                online_devices
                    .lock(device_id)
                    .wrap_err("failed to lock one of the inputs")?,
            );
        }

        notify_all_users(&format!("Starting break timer for {work_duration:?}"));
        thread::sleep(work_duration);

        for lock in locks {
            lock.unlock()?
        }
    }
}

fn block_on_new_input(recv_any_input: &Receiver<InputResult>) -> color_eyre::Result<()> {
    loop {
        match recv_any_input.try_recv() {
            Err(_) => break,
            Ok(Err(e)) => return Err(e).wrap_err("Error with device file"),
            Ok(Ok(_)) => (), // old event, ignore
        }
    }

    match recv_any_input.recv() {
        Err(_) => return Ok(()), // device disconnected
        Ok(Err(e)) => return Err(e).wrap_err("Error with device file"),
        Ok(Ok(_)) => return Ok(()), // new event! stop blocking
    }
}
