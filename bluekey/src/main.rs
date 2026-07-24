use std::{path::PathBuf, str::FromStr};

use bluer::Address;
use tokio::io::{AsyncBufReadExt, BufReader};
use zbus::proxy;
use clap::Parser;

#[proxy(
    interface="us.colbystuff.Bluekey1",
    default_service="us.colbystuff.Bluekey",
    default_path="/us/colbystuff/Bluekey"
)]
trait Bluekey {
    async fn bridge_mouse(&self, mouse: &PathBuf, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn bridge_keyboard(&self, mouse: &PathBuf, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn destroy_bridge(&self, handle: u64) -> Result<(), zbus::fdo::Error>;
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
  
    if let Err(Error(message, error)) = command(cli).await {
        println!("{}", message);
        println!("Error: {:?}", error);
    }

}

async fn manage<T, R, F: AsyncFnOnce(&T) -> R, M: AsyncFnOnce(T) -> ()>(value: T, next: F, manager: M) -> R {
    let result = next(&value).await;
    manager(value).await;
    result
}

async fn command(cli: Cli) -> Result<(), Error> {
    

    // Parse the address
    let address = Address::from_str(&cli.mac).map_err(|e| Error::new("Invalid MAC address", e))?;
    let address_str: String = format!("{}", address);

    // Get the Bluekey things required
    let connection = zbus::Connection::session().await.map_err(|e| Error::new("Unable to connect to DBus session", e))?;
    let proxy = BluekeyProxy::new(&connection).await.map_err(|e| Error::new("Unable to connect to Bluekey bus(is Bluekeyd running?)", e))?;
    
    let mouse_id = match cli.devices.mouse {
        Some(mouse) => Some(proxy.bridge_mouse(&mouse, &address_str).await.map_err(|e| Error::new("Error creating mouse bridge", e))?),
        None => None
    };
    manage(mouse_id, async |mouse_id| {
        let keyboard_id = match cli.devices.keyboard {
            Some(keyboard) => Some(proxy.bridge_keyboard(&keyboard, &address_str).await.map_err(|e| Error::new("Error creating keyboard bridge", e))?),
            None => None
        };
        manage(keyboard_id, async |keyboard_id| {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            println!("Press enter to end forwarding");
            lines.next_line().await.map_err(|e| Error::new("Error getting stdin", e))?;

            Ok(())
        }, async |id| match id {
            None => (),
            Some(id) => {proxy.destroy_bridge(id).await.unwrap();}
        }).await?;

        

        Ok(())
    }, async |id| match id {
        None => (),
        Some(id) => {proxy.destroy_bridge(id).await.unwrap();}
    }).await?;

    Ok(())
}