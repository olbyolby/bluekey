use std::{path::PathBuf, io::Write, fmt::Display, collections::HashMap, str::FromStr};

use bluer::{Address, InvalidAddress};
use futures::StreamExt;
use log::warn;
use clap::{Parser, Subcommand, Args};
use zvariant::{OwnedValue};

use crate::format::Groupable;

mod ev_key_map;
mod wait_enter;
mod format;
mod bridge;
mod commands;

use commands::bluekey::{self, Shortcut};

struct HandleWrapper<'a> {
    proxy: bluekey::BridgeProxy<'a>,
    handles: Vec<u64>
}
impl<'a> HandleWrapper<'a> {
    async fn wrap<F: AsyncFnOnce(&mut HandleWrapper) -> T, T>(proxy: bluekey::BridgeProxy<'a>, handler: F) -> T {
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
#[derive(Args)]
struct Bridge {
    #[clap(flatten)]
    input_device: InputDevices,
    #[clap(flatten)]
    remote_device: RemoteDeviceArgs,
    #[arg(long)]
    user: bool
}
#[derive(Args)]
#[group(required = true)]
struct InputDevices {

    #[arg(long)]
    /// Path to keyboard device to forward(/dev/input/*)
    keyboard: Option<PathBuf>,
    
    #[arg(long)]
    /// Path to mouse device to forward(/dev/input/*)
    mouse: Option<PathBuf>,
}
#[derive(Args)]
#[group(required = true, multiple = false)]
struct RemoteDeviceArgs {
    #[arg(long)]
    /// MAC address of device to bridge input to
    mac: Option<String>,
    #[arg(long)]
    /// Name/alias of device to bridge input to
    alias: Option<String>
}
impl RemoteDeviceArgs {
    fn resolve<'a>(&'a self) -> RemoteDevice<'a> {
        if let Some(mac) = &self.mac {
            return RemoteDevice::Mac(mac)
        } if let Some(alias) = &self.alias {
            return RemoteDevice::Alias(alias)
        }
        unreachable!("Clap should not allow more than 2 of these arguments")
    }
}
enum RemoteDevice<'a> {
    Mac(&'a str),
    Alias(&'a str)
}



#[derive(Args)]
struct List {
    #[arg(short, long)]
    // List details about each device(keyboard or mouse support)
    detailed: bool,
    #[arg(long)]
    user: bool
}
#[derive(Args)]
struct EscapeShortcut {
    shortcut: Option<String>,
    #[arg(long)]
    user: bool
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
enum ShortcutFormattingError<'a> {
    InvalidCharacter(char),
    InvalidKey(&'a str)
}
impl<'a> Display for ShortcutFormattingError<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCharacter(character) => write!(f, "Invalid character \"{}\" in shortcut.", character),
            Self::InvalidKey(name) => write!(f, "Unknown key \"{}\" in shortcut.", name)
        }
    }
}
impl<'a> From<ShortcutFormattingError<'a>> for Error<'a> {
    fn from(value: ShortcutFormattingError<'a>) -> Self {
        Self::ShortcutFormatting(value)
    }
}

#[derive(Clone)]
enum Error<'a> {
    AddressFormatting(&'a str, InvalidAddress),
    NoSuchAlias(&'a str),
    MultipleAliases(&'a str),
    ShortcutFormatting(ShortcutFormattingError<'a>),
    DbusConnection(&'static str, DBusError),
    BlueZError(&'static str, bluer::Error),
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
            Self::ShortcutFormatting(error) => error.fmt(f),
            Self::Keyboard(error) => Self::device_error(f, "keyboard", error),
            Self::Mouse(error) => Self::device_error(f, "mouse", error),
            Self::DbusConnection(stage, e) => write!(f, "DBus error while {}: {}", stage, e),
            Self::BlueZError(stage, e) => write!(f, "Bluer error while {}: {}", stage, e),
            Self::NoSuchAlias(alias) => write!(f, "No device found with alias \"{}\"", alias),
            Self::MultipleAliases(alias) => write!(f, "Multiple devices with alias \"{}\"", alias)
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let cli = Cli::parse();

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let result = match &cli.command {
        Commands::Bridge(args) => bridge(args).await,
        Commands::List(args) => list(args).await,
        Commands::EscapeShortcut(args) => escape_shortcut(args).await
    };
    if let Err(error) = result{
        println!("{}", error);
    }
}

async fn bridge<'a>(cli: &'a Bridge) -> Result<(), Error<'a>> { 
    // Establish DBus connection
    let connection = match !cli.user {
        true => zbus::Connection::system().await,
        false => zbus::Connection::session().await
    }.map_err(|e| Error::DbusConnection("establishing DBus connection", e.into()))?;

    let bridges = bluekey::BridgeProxy::new(&connection).await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;
    let config = bluekey::ConfigProxy::new(&connection).await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;

    // Establish BlueZ connection
    let session = bluer::Session::new().await.map_err(|e| Error::BlueZError("establishing BlueZ connection", e))?;
    let adapter = session.default_adapter().await.map_err(|e| Error::BlueZError("acquiring BlueZ adapter", e))?;

    // Parse the device's MAC address
    let (_target, address): (Address, std::borrow::Cow<'a, str>) = match cli.remote_device.resolve() {
        RemoteDevice::Mac(mac) => Address::from_str(mac).map(|address| (address, mac.into())).map_err(|e| Error::AddressFormatting(mac, e))?,
        RemoteDevice::Alias(alias) => {
            // Search all the devices for the one specified by the alias
            let devices = adapter.device_addresses().await.map_err(|e| Error::BlueZError("Getting device list", e))?;
            let mut found_device = None;
            for address in devices {
                let device = adapter.device(address).map_err(|e| Error::BlueZError("getting device handle", e))?;
                let name = device.alias().await.map_err(|e| Error::BlueZError("getting device name", e))?;

                if name == alias {
                    if found_device == None {
                        found_device = Some(address)
                    } else {
                        return Err(Error::MultipleAliases(alias))
                    }
                }
            }

            match found_device {
                None => return Err(Error::NoSuchAlias(alias)),
                Some(alias) => (alias, alias.to_string().into())
            }
        },
    };

    HandleWrapper::wrap(bridges, async |proxy| {
        let mut breakage = proxy.proxy.receive_bridge_broken().await.map_err(|e| Error::DbusConnection("listening for bridge breaks", e.into()))?;

        let mouse = match &cli.input_device.mouse {
            Some(mouse) => Some(proxy.bridge_mouse(&mouse, &address).await.map_err(|e| Error::Mouse(e))),
            None => None
        }.transpose()?;

        let keyboard = match &cli.input_device.keyboard {
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
    // Establish DBus connection and Bluer connection
    let connection = match !cli.user {
        true => zbus::Connection::system().await,
        false => zbus::Connection::session().await
    }.map_err(|e| Error::DbusConnection("establishing DBus connection", e.into()))?;

    let manager = zbus::fdo::ObjectManagerProxy::new(&connection, "us.colbystuff.Bluekey", "/us/colbystuff/Bluekey/devices").await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;
    
    let bluetooth_session = bluer::Session::new().await.map_err(|e| Error::BlueZError("connecting to BlueZ", e))?;
    let adapter = bluetooth_session.default_adapter().await.map_err(|e| Error::BlueZError("acquiring Bluetooth adapter", e))?;

    // Some constants for formatting things 
    const STAGE: &'static str = "reading device properties";
    const DIM: format::AnsiFormat<'static> = format::AnsiFormat::new("\x1B[2m", "\x1B[22m");
    const NONE: format::AnsiFormat<'static> = format::AnsiFormat::new("", "");

    // Checking if anything was listed
    let mut had_any = false;

    let mut entry = match cli.detailed {
        true => std::io::stdout().into_group("\n"),
        false => std::io::stdout().into_group(", ")
    };
    // List every device
    for (_, data) in manager.get_managed_objects().await.map_err(|e| Error::DbusConnection("reading active devices", e.into()))? {
        if let Some(interface) = data.get("us.colbystuff.Bluekey.Device1") {
            let address: &str = read_field(interface, "Address", STAGE)?;
            let keyboard: bool = read_field(interface, "HasKeyboard", STAGE)?;
            let mouse: bool  = read_field(interface, "HasMouse", STAGE)?;
            let power: u8 = read_field(interface, "Power", STAGE)?; 
            had_any = true;

            let entry = entry.next().unwrap();
            let address = format::AnsiFormat::wrap(match power {
                1 => DIM,
                _ => NONE,
            }, &address);

            match cli.detailed {
                false => write!(entry, "{}", address).unwrap(),
                true => {
                    // Acquire the name
                    let device = adapter.device(Address::from_str(&address).unwrap()).map_err(|e| Error::BlueZError("getting device handle", e))?;
                    let name = device.alias().await.map_err(|e| Error::BlueZError("getting device name", e))?;

                    write!(entry, "Address: {}, Name: {}; ", address, name).unwrap();
                
                    let mut devices = entry.group(", ");
                    if keyboard {
                        write!(devices.next().unwrap(), "Keyboard").unwrap();
                    }
                    if mouse {
                        write!(devices.next().unwrap(), "Mouse").unwrap();
                    }
                }
            }

        }
    }

    // Termiante the last part of the list(or display that there's none)
    if !had_any {
        println!("No devices connected.");
    } else {
        print!("\n");
    }

    std::io::stdout().flush().unwrap();

    Ok(())
}

async fn escape_shortcut<'a>(cli: &'a EscapeShortcut) -> Result<(), Error<'a>> {
    let shortcut = cli.shortcut.as_ref().map(|text| {
        let mut shortcut: Vec<u16> = Vec::new();
        for name in text.split("+") {
            let name = name.trim();
            if let Some(character) = name.chars().find(|c| !c.is_alphanumeric() && *c != '_') {
                return Err(ShortcutFormattingError::InvalidCharacter(character))
            }
            
            shortcut.push(ev_key_map::name_to_evdev_keycode(name).ok_or(ShortcutFormattingError::InvalidKey(name))?);
        }
        
        Ok(Shortcut::new(shortcut))
    }).transpose()?;


    let connection = match !cli.user {
        true => zbus::Connection::system().await,
        false => zbus::Connection::session().await
    }.map_err(|e| Error::DbusConnection("establishing DBus connection", e.into()))?;

    let config = bluekey::ConfigProxy::new(&connection).await.map_err(|e| Error::DbusConnection("connecting to Bluekey", e.into()))?;

    match shortcut {
        Some(shortcut) => {
            config.set_keyboard_escape_shortcut(shortcut).await.map_err(|e| Error::DbusConnection("setting keyboard shortcut", e.into()))?
        },
        None => {
            println!("Keyboard shortcut is: {}", config.keyboard_escape_shortcut().await.map_err(|e| Error::DbusConnection("reading escape shortcut", e.into()))?)
        }
    };

    Ok(())
}