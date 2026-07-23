#![allow(dead_code)] // Annoying 'cause I have unfinished APIs
use std::{collections::{HashMap, hash_map::Entry}, path::Path, str::FromStr, sync::Arc};

use bluer::Address;
// An actually half decent Bluetooth keyboard emulator
use evdev::Device;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use zbus::interface;
use std::path::PathBuf;
use clap::Parser;

use crate::bluetooth::{Target, keyboard::Keyboard, mouse::Mouse};

mod bluetooth;
mod evdev_bridge;

use evdev_bridge::{EvdevBridgeError, KeyboardBridge, MouseBridge};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
struct Id(u64);
impl zvariant::Type for Id {
    const SIGNATURE: &'static zvariant::Signature = &zvariant::signature!("(t)");
}
impl std::fmt::Display for Id {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "<{}>", self.0)?;
        Ok(())
    }
}


struct IdSource {
    id: u64
}
impl IdSource {
    fn new() -> Self {
        Self { id: 0 }
    }
    fn next(&mut self) -> Id {
        let id = self.id;
        self.id += 1;
        Id(id)
    }
}

enum Bridge {
    Keyboard(KeyboardBridge),
    Mouse(MouseBridge)
}
impl Bridge {
    async fn cancel(self) -> Result<(), EvdevBridgeError> {
        match self {
            Self::Keyboard(board) => board.cancel().await,
            Self::Mouse(mouse) => mouse.cancel().await
        }
    }
}

struct Bluekey {
    connection_id: IdSource,
    bridges: HashMap<Id, Bridge>,

    keyboard_server: Arc<Keyboard>,
    mouse_server: Arc<Mouse>
}

#[interface(name = "us.colbystuff.Bluekey1")]
impl Bluekey {
    async fn bridge_mouse(&mut self, mouse: &Path, mac: &str) -> Result<Id, zbus::fdo::Error> {
        let address = Address::from_str(mac).map_err(|_| zbus::fdo::Error::InvalidArgs("Invalid MAC address".into()))?;

        // Open and grab the device
        let mut device = evdev::Device::open(mouse).map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
        device.grab().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;

        // Start the bridge
        let bridge = MouseBridge::start(
            self.mouse_server.clone(), 
            device.into_event_stream().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?, 
            Target::Target(address)
        );
        
        // Acquire and store ID
        let id = self.connection_id.next();
        self.bridges.insert(id, Bridge::Mouse(bridge));

        info!("Started mouse bridge from {} to {} with handle {}", mouse.display(), address, id);
        Ok(id)
    }
    async fn destroy_bridge(&mut self, handle: Id) -> Result<(), zbus::fdo::Error> {
        let entry = match self.bridges.entry(handle) {
            Entry::Vacant(_) => Err(zbus::fdo::Error::Failed("No such handle".into())),
            Entry::Occupied(entry) => Ok(entry)
        }?;

        if let Err(error) = entry.remove().cancel().await {
            warn!("Bridge with handle {} failed with error: {:?}", handle, error);
        };

        info!("Destoryed bridge with handle {}", handle);
        Ok(())

    }

}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), zbus::Error> {
    env_logger::init();
    debug!("Test log");

    let session = bluer::Session::new().await.unwrap();
    let adapter = Arc::new(session.default_adapter().await.unwrap());

    let test = Bluekey { connection_id: IdSource::new(), bridges: HashMap::new(), keyboard_server: Arc::new(Keyboard::new(adapter.clone())), mouse_server: Arc::new(Mouse::new(adapter.clone()))};
    let connection = zbus::connection::Builder::session()?.name("us.colbystuff.Bluekey")?.serve_at("/us/colbystuff/Bluekey",test)?.build().await?;


    std::future::pending::<()>().await;
    drop(connection);
    Ok(())
}

/*
#[derive(Parser)]
#[command(name = "bluekeyd")]
/// Pass a keyboard or mouse through an emulated Bluetooth device
///
/// Emulate a Bluetooth keyboard or mouse service from this computer,
/// forwarding a keyboard or mouse on this device through it.
/// Enables sharing a mouse or keyboard with another device via Bluetooth,
/// without the need of a special app or software.
/// 
/// Note: You may need to run this program with the input group to access
/// evdev devices. An easy way to temporarily open a hash with this group is:
/// `sudo --preserve-env setpriv --regid $(id -g $USER) --reuid $(id -u $USER) --groups input,$(id -G $USER | sed "s/ /,/g") bash`
struct Cli {
    #[clap(flatten)]
    devices: Devices,

    #[arg(long)]
    /// Mac address of device to connect
    mac: Option<String>,

    #[arg(long)]
    /// Skip the short delay before grabing the keyboard(delay is to avoid a stuck enter key)
    skip_wait: bool
}
#[derive(clap::Args)]
#[group(required = true)]
struct Devices {

    #[arg(long, short)]
    /// Path to keyboard device to forward
    keyboard: Option<PathBuf>,
    
    #[arg(long, short)]
    /// Path to mouse device to forward
    mouse: Option<PathBuf>,
}

struct Aborter {
    task: tokio::task::JoinHandle<Result<std::convert::Infallible, EvdevBridgeError>>
}
impl Drop for Aborter {
    fn drop(&mut self) {
        self.task.abort(); // Summarily execute the task
    }
}

#[derive(Debug)]
struct Error(&'static str, Box<dyn std::fmt::Debug>);


async fn main() {
    env_logger::init();
    if let Err(Error(message, error)) = command().await {
        println!("{}", message);
        println!("Error: {:?}", error);
    }

}


async fn command() -> Result<(), Error> {
    let cli = Cli::parse();

    // A brief delay to avoid a stuck key(particularly enter) before grabbing keyboard
    if !cli.skip_wait {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Get address
    let address = cli.mac
        .map(|mac| Address::from_str(&mac)
            .map(|a| Target::Target(a))
            .map_err(|e| Error("Invalid Mac address formatting", Box::new(e))))
        .transpose()?.unwrap_or_else(|| {
            println!("Warning: With no MAC address specified, the server will use Broadcast Mode and ALL connected devices will recieve input simultaniously.");
            println!("Additionally, the keyboard will only display LEDs(Caps lock, num lock) for the first connected device.");
            Target::Broadcast
        });
        
    // Set up Bluetooth adapter and start evdev bridges
    let session = bluer::Session::new().await.map_err(|e| Error("Unable to open Bluetooth session(Is BlueZ running?)", Box::new(e)))?;
    let adapter = Arc::new(session.default_adapter().await.map_err(|e| Error("Unable to access Bluetooth adapter", Box::new(e)))?);

    let mouse = cli.devices.mouse.map(|device| {
        let mouse = Mouse::new(adapter.clone());
        let mut device = Device::open(device)?;
        device.grab()?;

        Ok(tokio::spawn(evdev_mouse_bridge(mouse, device, address)))
    }).transpose().map_err(|e: std::io::Error| Error("Error establishing Bluetooth mouse bridge", Box::new(e)))?.map(|task| Aborter { task });
    let keyboard = cli.devices.keyboard.map(|device| {
        let keyboard = Keyboard::new(adapter.clone());
        let mut device = Device::open(device)?;
        device.grab()?;

        Ok(tokio::spawn(evdev_keyboard_bridge(keyboard, device, address)))
    }).transpose().map_err(|e: std::io::Error| Error("Error establishing Bluetooth keyboard bridge", Box::new(e)))?;

    // If the keyboard is being used, exit via the keyboard(since you can't press enter if the keyboard is grabbed),
    // otherwise, wait for enter. 
    match keyboard {
        None => {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            println!("Press enter to end forwarding");
            lines.next_line().await.map_err(|e| Error("Error getting stdin", Box::new(e)))?;
        },
        Some(keyboard) => {
            println!("Type 'super/windows + esc' to break keyboard grab");
            keyboard.await.expect("Panic in keyboard server").map_err(|e| Error("Error in keyboard bridge", Box::new(e)))?;
        }
    }
    
    drop(mouse);

    Ok(())
}
*/