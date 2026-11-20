//! ogle: resource-efficient terminal system monitor (spec/).

mod alerts;
mod app;
mod cli;
mod config;
mod fmt;
mod modules;
mod platform;
mod term;
mod ui;

fn main() {
    std::process::exit(app::main());
}
