//! Core application modules for runtime orchestration, DB access, and Discord integration.

mod db;
mod discord;
mod embed;
mod i18n;
mod init;
mod listener;
mod runtime;
mod types;

pub(crate) use runtime::run;
