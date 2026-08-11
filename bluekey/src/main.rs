use std::collections::HashMap;
use std::fmt::Display;
use std::io::Write;
use std::path::Path;
use std::{path::PathBuf, str::FromStr};

use bluer::{Address, InvalidAddress};
use futures::StreamExt;
use log::warn;
use zbus::proxy;
use clap::{Parser, Subcommand, Args};
use zvariant::OwnedValue;

mod ev_key_map;
mod wait_enter;

#[proxy(
    interface="us.colbystuff.Bluekey.Bridge1",
    default_service="us.colbystuff.Bluekey",
    default_path="/us/colbystuff/Bluekey"
)]
trait BluekeyBridge {
    async fn bridge_mouse(&self, mouse: &Path, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn bridge_keyboard(&self, keyboard: &Path, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn destroy_bridge(&self, handle: u64) -> Result<(), zbus::fdo::Error>;

    #[zbus(signal)]
    fn bridge_broken(&self, id: u64) -> zbus::Result<()>;
}
#[proxy(
    interface="us.colbystuff.Bluekey.Configuration1",
    default_service="us.colbystuff.Bluekey",
    default_path="/us/colbystuff/Bluekey"
)]
trait BluekeyConfig {
    #[zbus(property)]
    fn keyboard_escape_shortcut(&self) -> Result<Shortcut, zbus::fdo::Error>;

}
#[proxy(
    interface="us.colbystuff.Bluekey.Device1",
    default_service="us.colbystuff.Bluekey"
)]
trait BluekeyDevice {
    #[zbus(property)]
    fn address(&self) -> Result<String, zbus::fdo::Error>;

    #[zbus(property)]
    fn has_keyboard(&self) -> Result<bool, zbus::fdo::Error>;

    #[zbus(property)]
    fn has_mouse(&self) -> Result<bool, zbus::fdo::Error>;
}



struct HandleWrapper<'a> {
    proxy: BluekeyBridgeProxy<'a>,
    handles: Vec<u64>
}
impl<'a> HandleWrapper<'a> {
    async fn wrap<F: AsyncFnOnce(&mut HandleWrapper) -> T, T>(proxy: BluekeyBridgeProxy<'a>, handler: F) -> T {
        let mut wrapper = Self {
            proxy,
            handles: Vec::new()
        };

        let result = handler(&mut wrapper).await;
        for handle in wrapper.handles {
            if let Err(error) = wrapper.proxy.destroy_bridge(handle).await {
                warn!("Error destroying bridge with handle {}: {}", handle, error);
            }
        };

        result
    }

    async fn bridge_mouse(&mut self, mouse: &PathBuf, mac: &str) -> Result<u64, zbus::fdo::Error> {
        let handle = self.proxy.bridge_mouse(mouse, mac).await?;
        self.handles.push(handle);
        Ok(handle)
    }

    async fn bridge_keyboard(&mut self, keyboard: &PathBuf, mac: &str) -> Result<u64, zbus::fdo::Error> {
        let handle = self.proxy.bridge_keyboard(keyboard, mac).await?;
        self.handles.push(handle);
        Ok(handle)
    }
}

struct Shortcut {
    keys: Vec<u16>
}
impl TryFrom<zvariant::OwnedValue> for Shortcut {
    type Error = zvariant::Error;
    fn try_from(value: zvariant::OwnedValue) -> Result<Self, Self::Error> {
        Ok(Self { keys: Vec::<u16>::try_from(value)?})
    }
}
impl Display for Shortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys = self.keys.iter();
        if let Some(key) = keys.next() {
            match ev_key_map::evdev_keycode_to_name(*key) {
                Some(name) => write!(f, "{}", name),
                None => write!(f, "<keycode: {}>", key)
            }?
        }

        for key in keys {
            match ev_key_map::evdev_keycode_to_name(*key) {
                Some(name) => write!(f, "+{}", name),
                None => write!(f, "+<keycode: {}>", key)
            }?
        };
        Ok(())
    }
}


#[derive(Parser)]
#[command(name = "bluekey")]
/// CLI interface for Bluekey, a Bluetooth keyboard/mouse emulator
struct Cli {
    #[command(subcommand)]
    command: Commands
}
#[derive(Subcommand)]
enum Commands {
    /// Pass a keyboard or mouse through an emulated Bluetooth device
    ///
    /// Note: You may need to run this program with the input group to access
    /// evdev devices. An easy way to temporarily open a hash with this group is:
    /// `sudo --preserve-env setpriv --regid $(id -g $USER) --reuid $(id -u $USER) --groups input,$(id -G $USER | sed "s/ /,/g") bash`
    Bridge(Bridge),
    /// List all devices known to Bluekey as listening for keyboard or mouse input
    List(List)
}
#[derive(Args)]
struct Bridge {
    #[clap(flatten)]
    devices: Devices,
    #[arg(long)]
    /// Mac address of device to connect
    mac: String,
}
#[derive(Args)]
struct List {
    #[arg(short, long)]
    // List details about each device(keyboard or mouse support)
    detailed: bool
}

#[derive(Args)]
#[group(required = true)]
struct Devices {

    #[arg(long)]
    /// Path to keyboard device to forward
    keyboard: Option<PathBuf>,
    
    #[arg(long)]
    /// Path to mouse device to forward
    mouse: Option<PathBuf>,
}

#[derive(Clone)]
enum DBusError {
    InvalidProperty,
    Zbus(zbus::fdo::Error)
}
impl From<zbus::fdo::Error> for DBusError {
    fn from(value: zbus::fdo::Error) -> Self {
        Self::Zbus(value)
    }
}
impl From<zbus::Error> for DBusError {
    fn from(value: zbus::Error) -> Self {
        Self::Zbus(value.into())
    }
}
impl Display for DBusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zbus(error) => error.fmt(f),
            Self::InvalidProperty => write!(f, "Recieved missing or invalid property from Bluekey.")
        }
    }
}

#[derive(Clone)]
enum Error<'a> {
    AddressFormatting(&'a str, InvalidAddress),
    DbusConnection(&'static str, DBusError),
    Keyboard(zbus::fdo::Error),
    Mouse(zbus::fdo::Error),
}
impl<'a> Error<'a> {
    fn device_error(formater: &mut std::fmt::Formatter<'_>, device: &'static str, error: &zbus::fdo::Error) -> std::fmt::Result {
        use zbus::fdo::Error;
        
        match error {
            Error::AccessDenied(error) => write!(formater, "Access denied creating {} bridge: {}", device, error),
            Error::IOError(error) => write!(formater, "IO Error creating {} bridge: {}", device, error),
            error => write!(formater, "Dbus error creating {}: {}", device, error)
        }
    }
}

impl<'a> Display for Error<'a> {
   
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddressFormatting(address, error) => write!(f, "Invalid text \"{}\" in address \"{}\"", error.0, address),
            Self::Keyboard(error) => Self::device_error(f, "keyboard", error),
            Self::Mouse(error) => Self::device_error(f, "mouse", error),
            Self::DbusConnection(stage, e) => write!(f, "DBus error while {}: {}", stage, e),
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::Bridge(args) => bridge(args).await,
        Commands::List(args) => list(args).await
    };
    if let Err(error) = result{
        println!("{}", error);
    }
}


async fn bridge<'a>(cli: &'a Bridge) -> Result<(), Error<'a>> { 
    // Parse the device's MAC address
    let (_target, address) = Address::from_str(&cli.mac).map(|address| (address, &cli.mac)).map_err(|e| Error::AddressFormatting(&cli.mac, e))?;

    // Establish DBus connection
    let connection = zbus::Connection::session().await.map_err(|e| Error::DbusConnection("establishing DBus connection", e.into()))?;
    let bridges = BluekeyBridgeProxy::new(&connection).await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;
    let config = BluekeyConfigProxy::new(&connection).await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;

    HandleWrapper::wrap(bridges, async |proxy| {
        let mut breakage = proxy.proxy.receive_bridge_broken().await.map_err(|e| Error::DbusConnection("listening for bridge breaks", e.into()))?;

        let mouse = match &cli.devices.mouse {
            Some(mouse) => Some(proxy.bridge_mouse(&mouse, &address).await.map_err(|e| Error::Mouse(e))),
            None => None
        }.transpose()?;

        let keyboard = match &cli.devices.keyboard {
            Some(keyboard) => {
                println!("Press {} to break keyboard grab.", config.keyboard_escape_shortcut().await.map_err(|e| Error::Keyboard(e))?);
                Some(proxy.bridge_keyboard(&keyboard, &address).await.map_err(|e| Error::Keyboard(e)))
            },
            None => None
        }.transpose()?;
        
        let mut stdin = std::io::stdin().lock();
  
        let mut next = async move || {
            tokio::select! {
                signal = breakage.next() => signal,
                _ = wait_enter::async_wait_enter(&mut stdin) => None
            }
        };

        println!("Press enter to exit.");
        while let Some(signal) = next().await {
            let args = match signal.args() {
                Ok(arg) => arg,
                Err(_) => {
                    warn!("Invalid signal from Bluekey");
                    continue;
                }
            };
            
            if keyboard == Some(args.id) || mouse == Some(args.id) {
                break      
            }

        }
        
        Ok::<(), Error>(())        
    }).await?;

    println!("Exiting...");

    Ok(())
}

fn read_field<'a, 'b, T: TryFrom<&'b OwnedValue>>(properties: &'b HashMap<String, OwnedValue>, name: &str, stage: &'static str) -> Result<T, Error<'a>> {
    let error = Error::DbusConnection(stage, DBusError::InvalidProperty);
    Ok(properties.get(name).ok_or(error.clone())?.try_into().map_err(|_| error)?)
}

async fn list<'a>(cli: &'a List) -> Result<(), Error<'a>> {
    // Establish DBus connection
    let connection = zbus::Connection::session().await.map_err(|e| Error::DbusConnection("establishing DBus connection", e.into()))?;
    let manager = zbus::fdo::ObjectManagerProxy::new(&connection, "us.colbystuff.Bluekey", "/us/colbystuff/Bluekey/devices").await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;

    const STAGE: &'static str = "reading device properties";
    for (_, data) in manager.get_managed_objects().await.map_err(|e| Error::DbusConnection("reading active devices", e.into()))? {
        if let Some(interface) = data.get("us.colbystuff.Bluekey.Device1") {
            let address: &str = read_field(interface, "Address", STAGE)?;
            let keyboard: bool = read_field(interface, "HasKeyboard", STAGE)?;
            let mouse: bool  = read_field(interface, "HasMouse", STAGE)?;

            match cli.detailed {
                false => print!("{}", address),
                true => {
                    print!("Address: {}, ", address);
                    if keyboard {
                        print!("Keyboard");
                    }
                    if mouse {
                        print!("Mouse");
                    }
                }
            }

            print!("\n");
        }
    }
    std::io::stdout().flush().unwrap();

    Ok(())
}
