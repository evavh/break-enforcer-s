use color_eyre::eyre::{eyre, Context};
use color_eyre::{Result, Section};
use serde::{Deserialize, Serialize};

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::watch::InputId;

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct InputFilter {
    pub id: InputId,
    /// empty name means block everything
    pub names: Vec<String>,
}

fn setup_default_path() -> PathBuf {
    let dir = Path::new(concat!("/etc/", env!("CARGO_CRATE_NAME"), ".ron"));
    assert!(
        dir.parent().expect("path has two components").is_dir(),
        "/etc should exist on unix"
    );
    dir.to_path_buf()
}

pub(crate) fn read(custom_path: Option<PathBuf>) -> Result<Vec<InputFilter>> {
    let path = custom_path.unwrap_or_else(setup_default_path);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err)
                .wrap_err("Could not read config which might exist")
                .with_note(|| format!("path: {}", path.display()))
        }
    };

    let s = String::from_utf8(bytes).wrap_err("Corrupt config, contained non utf8")?;
    ron::from_str(&s).wrap_err("Could not deserialize to list of devices")
}

pub(crate) fn write(to_lock: &[InputFilter], custom_path: Option<PathBuf>) -> Result<()> {
    let data = ron::ser::to_string_pretty(&to_lock, ron::ser::PrettyConfig::default())
        .wrap_err("Could not serialize list of devices to toml")?;

    let path = custom_path.unwrap_or_else(setup_default_path);
    if let Some(dir) = path.parent() {
        if !dir.is_dir() {
            return Err(
                eyre!("Dir does not exist!").with_note(|| format!("dir: {}", dir.display()))
            );
        }
    }

    fs::write(path, data.as_bytes()).wrap_err("Could not write serialized list to file")
}
