use std::{collections::HashMap, io::Write, str::FromStr};

use bluer::Address;
use clap::Args;
use zvariant::OwnedValue;


use crate::{commands::ToBluerError, format::{self, Groupable}};

use super::{bluekey::{ConnectionBusArgument}, Error};

#[derive(Args)]
pub struct List {
    #[clap(flatten)]
    bus: ConnectionBusArgument,


    #[arg(short, long)]
    // List details about each device
    long: bool,

}

impl List{
    pub async fn execute(&self)  -> Result<(), Error<'_>>{
        let connection = self.bus.connect().await?;

        let manager = zbus::fdo::ObjectManagerProxy::new(&connection, "us.colbystuff.Bluekey", "/us/colbystuff/Bluekey/devices").await?;

        let bluetooth_session = bluer::Session::new().await.connecting_error()?;
        let adapter = bluetooth_session.default_adapter().await.method_error()?;        

        // Some constants for formatting things 
        const DIM: format::AnsiFormat<'static> = format::AnsiFormat::new("\x1B[2m", "\x1B[22m");
        const NONE: format::AnsiFormat<'static> = format::AnsiFormat::new("", "");

        // Checking if anything was listed
        let mut had_any = false;

        let mut entry = match self.long {
            true => std::io::stdout().into_group("\n"),
            false => std::io::stdout().into_group(", ")
        };
        // List every device
        for (_, data) in manager.get_managed_objects().await? {
            if let Some(interface) = data.get("us.colbystuff.Bluekey.Device1") {
                let address: &str = Self::read_field(interface, "Address");
                let keyboard: bool = Self::read_field(interface, "HasKeyboard");
                let mouse: bool  = Self::read_field(interface, "HasMouse");
                let power: u8 = Self::read_field(interface, "Power");
                had_any = true;

                let entry = entry.next().unwrap();
                let address = format::AnsiFormat::wrap(match power {
                    1 => DIM,
                    _ => NONE,
                }, &address);

                match self.long {
                    false => write!(entry, "{}", address).unwrap(),
                    true => {
                        // Acquire the name
                        let device = adapter.device(Address::from_str(&address).unwrap()).method_error()?;
                        let name = device.alias().await.method_error()?;

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
    fn read_field<'a, 'b, T: TryFrom<&'b OwnedValue>>(properties: &'b HashMap<String, OwnedValue>, name: &str) -> T 
      where T::Error: std::fmt::Debug {
        properties.get(name).unwrap().try_into().unwrap()
    }
}