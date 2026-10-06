use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct AccountLoginOptions {
    router_root: Option<PathBuf>,
    label: Option<String>,
    pub(super) provider: Option<Provider>,
}

impl AccountLoginOptions {
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--label" => {
                    options.label = Some(parser.next_required_value("--label")?);
                }
                "--provider" if options.provider.is_none() => {
                    let value = parser.next_required_value("--provider")?;
                    options.provider =
                        Some(
                            Provider::parse(&value).ok_or_else(|| CliError::UnknownOption {
                                option: format!("--provider {value}"),
                            })?,
                        );
                }
                "--provider" => {
                    return Err(AccountCommandError::DuplicateAccountLoginOption {
                        option: "--provider",
                    }
                    .into());
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(options)
    }

    pub(super) fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }

    pub(super) fn label(&self) -> Result<String, CliError> {
        self.label
            .clone()
            .ok_or(CliError::MissingOption { option: "--label" })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct AccountRootOptions {
    router_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct AccountSetWeeklyFloorOptions {
    router_root: Option<PathBuf>,
    account_label: Option<String>,
    percent: Option<u16>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct AccountStatusOptions {
    router_root: Option<PathBuf>,
    account_label: Option<String>,
}

impl AccountStatusOptions {
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();
        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" if options.router_root.is_none() => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--account" if options.account_label.is_none() => {
                    options.account_label = Some(parser.next_required_value("--account")?);
                }
                "--router-root" => {
                    return Err(AccountCommandError::DuplicateAccountStatusOption {
                        option: "--router-root",
                    }
                    .into());
                }
                "--account" => {
                    return Err(AccountCommandError::DuplicateAccountStatusOption {
                        option: "--account",
                    }
                    .into());
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }
        Ok(options)
    }

    pub(super) fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }

    pub(super) fn account_label(&self) -> Result<String, CliError> {
        self.account_label.clone().ok_or(CliError::MissingOption {
            option: "--account",
        })
    }
}

impl AccountSetWeeklyFloorOptions {
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();
        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    if options.router_root.is_some() {
                        return Err(AccountCommandError::DuplicateWeeklyFloorOption {
                            option: "--router-root",
                        }
                        .into());
                    }
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--account" => {
                    if options.account_label.is_some() {
                        return Err(AccountCommandError::DuplicateWeeklyFloorOption {
                            option: "--account",
                        }
                        .into());
                    }
                    options.account_label = Some(parser.next_required_value("--account")?);
                }
                "--percent" => {
                    if options.percent.is_some() {
                        return Err(AccountCommandError::DuplicateWeeklyFloorOption {
                            option: "--percent",
                        }
                        .into());
                    }
                    let raw_percent = parser.next_required_value("--percent")?;
                    let percent = raw_percent
                        .parse::<u16>()
                        .ok()
                        .filter(|percent| *percent <= 15)
                        .ok_or(AccountCommandError::InvalidWeeklyFloorPercent)?;
                    options.percent = Some(percent);
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }
        Ok(options)
    }

    pub(super) fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }

    pub(super) fn account_label(&self) -> Result<String, CliError> {
        self.account_label.clone().ok_or(CliError::MissingOption {
            option: "--account",
        })
    }

    pub(super) fn percent(&self) -> Result<u16, CliError> {
        self.percent.ok_or(CliError::MissingOption {
            option: "--percent",
        })
    }
}

impl AccountRootOptions {
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(options)
    }

    pub(super) fn router_root(self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root)
    }
}
