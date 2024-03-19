use clap::{Parser, Subcommand};
use std::num::ParseFloatError;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Periodically block devices in config (setup using wizard)
    Run {
        /// Period after which input will be disabled.
        /// run help for the format.
        #[arg(short, long, value_name = "work", value_parser = parse_duration)]
        work_duration: Duration,
        /// Length of the breaks, after this period input is resumed.
        /// run help for the format.
        #[arg(short, long, value_name = "break", value_parser = parse_duration)]
        break_duration: Duration,
        /// Duration ahead of the break to show a notification
        /// run help for the format.
        #[arg(short, long, value_name = "warn", value_parser = parse_duration)]
        grace_duration: Duration,
    },
    /// Pick the devices to block and write them to a config file.
    /// (Interactive UI)
    Wizard,
}

/// Disables specified input devices during breaks. The period between breaks,
/// length of the breaks and time before getting a warning can all be specified.
///
/// Durations can be passed in two formats:
///  - <amount><unit>, for example: 32m
///    unit is one of h,m and s
///  - hh:mm:ss, where hh and mm are optional
///    example: 1:30:15
///         one and a halve hour and 15 seconds
///    example: 10:40
///         ten minutes and 40 seconds
///
#[derive(Parser)]
#[command(version, about, verbatim_doc_comment)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    /// Path to create/read/update list of devices to/from
    #[arg(short, long)]
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Could not parse the second part of the time as number")]
    Second(ParseFloatError, String),
    #[error("Could not parse the minute part of the time as number")]
    Minute(ParseFloatError, String),
    #[error("Could not parse the minute part of the time as number")]
    Hour(ParseFloatError, String),
}

macro_rules! err_builder {
    ($name:ident, $variant:expr) => {
        fn $name(e: ParseFloatError, s: &str) -> ParseError {
            $variant(e, s.to_owned())
        }
    };
}

err_builder!(second_err, ParseError::Second);
err_builder!(minute_err, ParseError::Minute);
err_builder!(hour_err, ParseError::Hour);

/// Parses a string in format
///     hh:mm:ss,
///     mm:ss,
///     ss,
pub(crate) fn parse_colon_duration(arg: &str) -> Result<f32, ParseError> {
    let Some((rest, seconds)) = arg.rsplit_once(':') else {
        return arg.parse().map_err(|e| second_err(e, arg));
    };
    let mut seconds = seconds.parse().map_err(|e| second_err(e, arg))?;
    let Some((hours, minutes)) = rest.rsplit_once(':') else {
        return Ok(seconds);
    };
    seconds += 60.0 * minutes.parse::<f32>().map_err(|e| minute_err(e, minutes))?;
    if hours.is_empty() {
        return Ok(seconds);
    };
    seconds += 60.0 * 60.0 * hours.parse::<f32>().map_err(|e| hour_err(e, hours))?;
    Ok(seconds)
}

/// Parse a string in format hh:mm:ss to a `Duration`
pub(crate) fn parse_duration(arg: &str) -> Result<Duration, ParseError> {
    let seconds = if let Some(hours) = arg.strip_suffix('h') {
        60. * 60. * hours.parse::<f32>().map_err(|e| hour_err(e, hours))?
    } else if let Some(minutes) = arg.strip_suffix('m') {
        60. * minutes.parse::<f32>().map_err(|e| minute_err(e, minutes))?
    } else if let Some(seconds) = arg.strip_suffix('s') {
        seconds.parse::<f32>().map_err(|e| second_err(e, seconds))?
    } else {
        parse_colon_duration(arg)?
    };
    Ok(std::time::Duration::from_secs_f32(seconds))
}
