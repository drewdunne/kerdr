//! Kerdr: an opinionated kanban cockpit for driving herdr sessions.

pub mod accounts;
pub mod actions;
pub mod app;
pub mod board;
pub mod config;
pub mod demo;
pub mod edit;
pub mod errlog;
pub mod family;
pub mod feed;
pub mod herdr;
pub mod herdr_keys;
pub mod hold;
pub mod images;
pub mod jump;
pub mod model;
pub mod owners;
pub mod palette;
pub mod poll;
pub mod providers;
pub mod report;
pub mod resolve;
pub mod stages;
pub mod state;
pub mod theme;
pub mod transport;
pub mod tui;
pub mod ui;

#[cfg(windows)]
pub mod windows;
