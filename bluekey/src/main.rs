use std::{path::PathBuf, str::FromStr, sync::Arc};

use bluer::Address;
use evdev::{Device, KeyCode};
use futures::StreamExt;
use log::warn;
use tokio::io::{AsyncBufReadExt, BufReader};
use zbus::proxy;
use clap::Parser;

use blueshare::evdev_bridge::{KeyboardBridge, MouseBridge, Shortcut};
use blueshare::bluetooth::{keyboard::Keyboard, mouse::Mouse, Target};

#[proxy(
    interface="us.colbystuff.Bluekey.Bridge1",
    default_service="us.colbystuff.Bluekey",
    default_path="/us/colbystuff/Bluekey"
)]
trait Bluekey {
    async fn bridge_mouse(&self, mouse: &PathBuf, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn bridge_keyboard(&self, keyboard: &PathBuf, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn destroy_bridge(&self, handle: u64) -> Result<(), zbus::fdo::Error>;

    #[zbus(signal)]
    fn bridge_broken(&self, id: u64) -> zbus::Result<()>;
}


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
    mac: String,

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

struct BridgeHandle {
    handle: Option<u64>
}
impl BridgeHandle {
    fn new() -> Self {
        Self { handle: None }
    }
    async fn destroy(self, proxy: &BluekeyProxy<'_>) {
        if let Some(handle) = self.handle {
            if let Err(e) = proxy.destroy_bridge(handle).await {
                warn!("Failed to free bridge with handle <{}>: {}", handle, e);
            }
        }
    }
    fn wrap<'a>(&'a mut self) -> Handler<'a> {
        Handler { handler: self}
    }
}
struct Handler<'a> {
    handler: &'a mut BridgeHandle
}
impl<'a> Handler<'a> {
    fn wrap(self, id: u64) {
        self.handler.handle = Some(id); 
    }
}


#[derive(Debug)]
struct Error(String, Box<dyn std::fmt::Debug>);
impl Error {
    fn new<M: Into<String>, E: std::fmt::Debug + 'static>(message: M, error: E) -> Self {
        Self(message.into(), Box::new(error))
    }
}


#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();
    let cli = Cli::parse();

    if !cli.skip_wait {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
  
    if let Err(Error(message, error)) = command(cli).await {
        println!("{}", message);
        println!("Error: {:?}", error);
    }

}

async fn command(cli: Cli) -> Result<(), Error> {
    // Parse the address
    let (target, address) = Address::from_str(&cli.mac).map(|address| (Target::Target(address), cli.mac)).map_err(|e| Error::new("Invalid MAC address", e))?;

    // Attempt to open a DBus connection to Bluekey
    let connection = async {
        let connection = zbus::Connection::session().await.map_err(|e| Error::new("Unable to connect to DBus session", e));
        match connection {
            Ok(connection) => match BluekeyProxy::new(&connection).await {
                Ok(proxy) => {
                    let dbus = zbus::fdo::DBusProxy::new(&connection).await.map_err(|e| Error::new("Could not connect to DBus's bus.", e))?;
                    match dbus.name_has_owner(zbus::names::BusName::from_static_str("us.colbystuff.Bluekey").unwrap()).await.map_err(|e| Error::new("Error accessing DBus", e))? {
                        true => Ok((connection, proxy)),
                        false => Err(Error::new("Unable to connect to DBus daemon(is Bluekey running?)", ()))
                    }
                },
                Err(error) => Err(Error::new("Unable to connect to Bluekey daemon(is Bluekey running?)", error))
            }
            Err(error) => Err(error)
        }
    }.await;

    // Prepare for input 
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    match connection {
        Err(_) => {
            println!("Found no running daemon, running standalone");
            let session = bluer::Session::new().await.map_err(|e| Error::new("Error opening BlueZ session(Is BlueZ running?)", e))?;
            let adapter = Arc::new(session.default_adapter().await.map_err(|e| Error::new("Error opening Bluetooth adapter.", e))?);
            
            let keyboard = cli.devices.keyboard.map(|keyboard| {
                let mut device = Device::open(keyboard).map_err(|e| Error::new("Error opening keyboard device.", e))?;
                device.grab().map_err(|e| Error::new("Error grabing keyboard.", e))?;

                println!("Use Super/Windows + esc to break keyboard grab");
                let keyboard = Keyboard::new(adapter.clone());
                Ok(KeyboardBridge::start(keyboard, device.into_event_stream().map_err(|e| Error::new("Error generating keyboard stream", e))?, target, Shortcut::new(Arc::new(Vec::from([KeyCode(1), KeyCode(125)]))), async || ()))
            }).transpose()?;

            let mouse = cli.devices.mouse.map(|mouse| {
                let mut device = Device::open(mouse).map_err(|e| Error::new("Error opening mouse device.", e))?;
                device.grab().map_err(|e| Error::new("Error grabbing mouse.", e))?;

                let mouse = Mouse::new(adapter.clone());
                Ok(MouseBridge::start(mouse, device.into_event_stream().map_err(|e| Error::new("Error generating mouse stream", e))?, target))
            }).transpose()?;


            match keyboard {
                Some(board) => board.wait_for_break().await.map_err(|e| Error::new("Error in keyboard bridge", e)).map(|_| ()),
                None => {
                    println!("Press enter to exit.");
                    lines.next_line().await.map_err(|e: std::io::Error| Error::new("Stdin input error.",e)).map(|_| ())
                }
            }?;

            drop(mouse);
            Ok(())
        },
        Ok((_connection, proxy)) => {
            let proxy2 = proxy.clone();
            tokio::task::spawn(async move {
                let mut stream = proxy2.receive_bridge_broken().await.unwrap();
                println!("Starting");
                while let Some(msg) = stream.next().await {
                    let args = msg.args().unwrap();
                    println!("{:?}", args.id);
                }
                println!("Stream is none?");
            });

            let mut keyboard = BridgeHandle::new();
            let mut mouse = BridgeHandle::new();
            
            let result: Result<(), Error>  = (async |keyboard: Handler<'_>, mouse: Handler<'_>| {
                // Create the 2 handles
                if let Some(board) = cli.devices.keyboard {
                    keyboard.wrap(proxy.bridge_keyboard(&board, &address).await.map_err(|e| Error::new("Error creating keyboard bridge.",e))?)
                }
                if let Some(rat) = cli.devices.mouse {
                    mouse.wrap(proxy.bridge_mouse(&rat, &address).await.map_err(|e| Error::new("Error creating mouse bridge", e))?)
                }

                
                Ok(())
            })(keyboard.wrap(), mouse.wrap()).await;
            keyboard.destroy(&proxy).await;
            mouse.destroy(&proxy).await;

            result
        }
    }
}

