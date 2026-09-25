//! Owner-editable provider defaults read once at each Host start.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigurationEntry {
    pub enabled: bool,
    pub executable: Option<PathBuf>,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigurationEntries {
    version: u8,
    providers: ProviderEntries,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderEntries {
    claude: ProviderConfigurationEntry,
    cursor: ProviderConfigurationEntry,
}

impl ProviderConfigurationEntries {
    #[must_use]
    pub fn claude(&self) -> &ProviderConfigurationEntry {
        &self.providers.claude
    }

    #[must_use]
    pub fn cursor(&self) -> &ProviderConfigurationEntry {
        &self.providers.cursor
    }
}

#[derive(Debug, Error)]
pub enum ProviderConfigurationError {
    #[error("cannot read or create {path}: {source}")]
    FileIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("malformed provider configuration {path}: {message}")]
    Malformed { path: PathBuf, message: String },
}

pub struct ProviderConfigurationFile;

impl ProviderConfigurationFile {
    pub fn load_or_create(
        router_root: &Path,
    ) -> Result<ProviderConfigurationEntries, ProviderConfigurationError> {
        let path = router_root.join("providers.json");
        match fs::read(&path) {
            Ok(bytes) => Self::parse(&path, &bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(router_root).map_err(|source| {
                    ProviderConfigurationError::FileIo {
                        path: path.clone(),
                        source,
                    }
                })?;
                let defaults = ProviderConfigurationEntries::default();
                let mut encoded = serde_json::to_vec_pretty(&defaults).map_err(|error| {
                    ProviderConfigurationError::Malformed {
                        path: path.clone(),
                        message: error.to_string(),
                    }
                })?;
                encoded.push(b'\n');
                use std::io::Write as _;
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        file.write_all(&encoded)
                            .and_then(|()| file.sync_all())
                            .map_err(|source| ProviderConfigurationError::FileIo {
                                path: path.clone(),
                                source,
                            })?;
                        Ok(defaults)
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        let bytes = fs::read(&path).map_err(|source| {
                            ProviderConfigurationError::FileIo {
                                path: path.clone(),
                                source,
                            }
                        })?;
                        Self::parse(&path, &bytes)
                    }
                    Err(source) => Err(ProviderConfigurationError::FileIo { path, source }),
                }
            }
            Err(source) => Err(ProviderConfigurationError::FileIo { path, source }),
        }
    }

    fn parse(
        path: &Path,
        bytes: &[u8],
    ) -> Result<ProviderConfigurationEntries, ProviderConfigurationError> {
        let configuration: ProviderConfigurationEntries =
            serde_json::from_slice(bytes).map_err(|error| {
                ProviderConfigurationError::Malformed {
                    path: path.to_owned(),
                    message: error.to_string(),
                }
            })?;
        if configuration.version != 1 {
            return Err(ProviderConfigurationError::Malformed {
                path: path.to_owned(),
                message: format!("unsupported version {} (expected 1)", configuration.version),
            });
        }
        Ok(configuration)
    }
}

impl Default for ProviderConfigurationEntries {
    fn default() -> Self {
        Self {
            version: 1,
            providers: ProviderEntries {
                claude: ProviderConfigurationEntry {
                    enabled: true,
                    executable: None,
                    arguments: Vec::new(),
                },
                cursor: ProviderConfigurationEntry {
                    enabled: true,
                    executable: None,
                    arguments: vec!["acp".to_owned()],
                },
            },
        }
    }
}
