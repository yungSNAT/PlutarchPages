// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Standalone Unix & Web packaging orchestrator (`rustc --edition=2024 -D warnings`).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod unix_package_cert;
mod unix_package_env;
mod unix_package_flatpak;
mod unix_package_freebsd;
mod unix_package_fs;
mod unix_package_linux;
mod unix_package_macos;
#[cfg(test)]
mod unix_package_tests;
mod unix_package_web;

use std::env;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use unix_package_env::EnvCtx;

#[derive(Debug)]
pub struct AppError {
    pub message: String,
    pub code: u8,
}

impl AppError {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: 1 }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: 2 }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::runtime(err.to_string())
    }
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        Self::runtime(message)
    }
}

impl From<&str> for AppError {
    fn from(message: &str) -> Self {
        Self::runtime(message)
    }
}

pub type AppResult<T = ()> = Result<T, AppError>;

fn parse_root_and_command(args: &[String]) -> AppResult<(Option<PathBuf>, &str, &[String])> {
    let (root, rest) = match args {
        [flag, path, tail @ ..] if flag == "--root" => {
            if path.is_empty() {
                return Err(AppError::usage("error: --root requires a non-empty path"));
            }
            (Some(PathBuf::from(path)), tail)
        }
        [flag] if flag == "--root" => {
            return Err(AppError::usage("error: --root requires a path"));
        }
        _ => (None, args),
    };
    let Some((cmd, cmd_args)) = rest.split_first() else {
        return Err(AppError::usage("usage: unix_package [--root DIR] <linux|freebsd|macos|flatpak|web|import-cert> [args...]"));
    };
    Ok((root, cmd.as_str(), cmd_args))
}

fn dispatch(args: &[String]) -> AppResult {
    let (root_override, cmd, cmd_args) = parse_root_and_command(args)?;
    let env_ctx = EnvCtx::load(root_override)?;
    match cmd {
        "linux" => unix_package_linux::run(&env_ctx, cmd_args),
        "freebsd" => unix_package_freebsd::run(&env_ctx, cmd_args),
        "macos" => unix_package_macos::run(&env_ctx, cmd_args),
        "flatpak" => unix_package_flatpak::run(&env_ctx, cmd_args),
        "web" => unix_package_web::run(&env_ctx, cmd_args),
        "import-cert" => unix_package_cert::run(&env_ctx, cmd_args),
        other => Err(AppError::usage(format!("unknown subcommand: {other}"))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if !err.message.is_empty() {
                eprintln!("{}", err.message);
            }
            ExitCode::from(err.code)
        }
    }
}
