//! Preserved standalone Sessions parser, catalog and picker contracts.
use crate::CliContext;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Duration, Instant},
};
mod session_test_support;
use session_test_support::*;
mod argument_contract_tests;
mod catalog_filter_tests;
mod catalog_launch_tests;
mod picker_contract_tests;
