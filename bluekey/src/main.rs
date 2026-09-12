use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod ev_key_map;
mod wait_enter;
mod format;
mod commands;

use commands::{
    bridge::Bridge,
    list::List,
    shortcut::EscapeShortcut
};

#[derive(Parser)]
#[command(name = "bluekey")]
/// CLI interface for Bluekey, a Bluetooth keyboard/mouse emulator
struct Cli {
    #[command(subcommand)]
    command: Commands
}
#[derive(Subcommand)]
enum Commands {
    /// Pass a keyboard or mouse through to a Bluetooth device
    /// 
    /// Bridges a physical keyboard and or mouse device over Bluetooth to a connected Bluetooth client, grabbing the keyboard/mouse 
    /// from the OS, as if the keyboard/mouse were connected directly to the Bluetooth device. 
    /// Your keyboard/mouse's device file(/dev/input/*) can be found with `sudo evtest`.
    /// 
    Bridge(Bridge),
    /// List all devices known to Bluekey as listening for keyboard or mouse input
    List(List),
    /// Set or view the keyboard escape shortcut, used for breaking the keyboard grab from the keyboard.
    /// Shortcut formatted as evdev key names seperated by '+'(ex: LEFTMETA+ESC)
    EscapeShortcut(EscapeShortcut)
}

#[tokio::main]
async fn main() -> ExitCode {
    env_logger::init();
    let cli = Cli::parse();

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let result = match &cli.command {
        Commands::Bridge(bridge) => bridge.execute().await,
        Commands::List(list) => list.execute().await,
        Commands::EscapeShortcut(shortcut) => shortcut.execute().await
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            println!("{}", error);
            ExitCode::FAILURE
        }
    }

}