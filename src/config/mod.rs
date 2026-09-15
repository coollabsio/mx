pub mod model;

use crate::config::model::ConfigV10;
use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use tempfile::NamedTempFile;

static CONFIG_DIR: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

pub fn configure_dir(dir: Option<PathBuf>) {
    *CONFIG_DIR
        .get_or_init(|| RwLock::new(None))
        .write()
        .expect("config dir lock poisoned") = dir;
}

fn configured_dir() -> Option<PathBuf> {
    CONFIG_DIR
        .get_or_init(|| RwLock::new(None))
        .read()
        .expect("config dir lock poisoned")
        .clone()
}

pub const CONFIG_FILE_NAME: &str = "config.json";
const MX_DIR_NAME: &str = ".mx";
const MC_DIR_NAME: &str = ".mc";

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    config: ConfigV10,
}

impl ConfigStore {
    pub fn load_or_create() -> Result<Self> {
        if let Some(dir) = configured_dir() {
            let path = dir.join(CONFIG_FILE_NAME);
            if path.exists() {
                return Self::load_from_path(path);
            }
            let store = Self {
                path,
                config: ConfigV10::new_with_defaults(),
            };
            store.save()?;
            return Ok(store);
        }

        let home = home_dir()?;
        let mx_path = home.join(MX_DIR_NAME).join(CONFIG_FILE_NAME);
        let mc_path = home.join(MC_DIR_NAME).join(CONFIG_FILE_NAME);

        if mx_path.exists() {
            return Self::load_from_path(mx_path);
        }
        if mc_path.exists() {
            return Self::load_from_path(mc_path);
        }

        let store = Self {
            path: mx_path,
            config: ConfigV10::new_with_defaults(),
        };
        store.save()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config(&self) -> &ConfigV10 {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut ConfigV10 {
        &mut self.config
    }

    pub fn save(&self) -> Result<()> {
        save_config(&self.path, &self.config)
    }

    fn load_from_path(path: PathBuf) -> Result<Self> {
        let file = fs::File::open(&path)
            .with_context(|| format!("Unable to open config file `{}`.", path.display()))?;
        let reader = BufReader::new(file);
        let config: ConfigV10 = serde_json::from_reader(reader)
            .with_context(|| format!("Unable to parse config file `{}`.", path.display()))?;

        if config.version != model::CONFIG_VERSION {
            bail!(
                "Unsupported config version `{}` in `{}`.",
                config.version,
                path.display()
            );
        }

        Ok(Self { path, config })
    }
}

pub fn save_config(path: &Path, config: &ConfigV10) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid config path `{}`.", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Unable to create config dir `{}`.", parent.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
    }

    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("Unable to create temp config in `{}`.", parent.display()))?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, config)
            .with_context(|| format!("Unable to serialize config for `{}`.", path.display()))?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    temp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("Unable to persist config to `{}`.", path.display()))?;
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }
    dirs::home_dir().ok_or_else(|| anyhow!("Unable to determine home directory."))
}
