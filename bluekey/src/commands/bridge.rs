use std::{panic::AssertUnwindSafe, path::PathBuf, str::FromStr};

use bluer::{Adapter, Address};
use futures::StreamExt;
use clap::Args;
use log::warn;


use crate::commands::AddressError;

use super::{bluekey::{self, ConnectionBusArgument}, Error};


#[derive(Args)]
pub struct Bridge {
    #[clap(flatten)]
    bus: ConnectionBusArgument,

    #[clap(flatten)]
    targets: DeviceTarget,

    #[clap(flatten)]
    remote_device: RemoteDeviceArgs,
   
}
#[derive(Args)]
#[group(required = true)]
struct DeviceTarget {
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
        } else if let Some(alias) = &self.alias {
            return RemoteDevice::Alias(alias)
        }
        unreachable!("Clap should not allow more than 2 of these arguments")
    }
}

#[derive(Clone, Copy)]
enum RemoteDevice<'a> {
    Mac(&'a str),
    Alias(&'a str)
}

struct Bridges {
    keyboard: Option<u64>,
    mouse: Option<u64>
}
impl Bridges {
    async fn wrap<F: AsyncFnOnce(&mut Self) -> R, R>(connection: bluekey::BridgeProxy<'_>,function: F) -> R {
        let mut handles = Bridges {
            keyboard: None,
            mouse: None
        };

        use futures::FutureExt;
        let result =  AssertUnwindSafe(function(&mut handles)).catch_unwind().await;
        
        if let Some(keyboard) = handles.keyboard {
            let _ = connection.destroy_bridge(keyboard).await;
        }
        if let Some(mouse) = handles.mouse {
            let _ = connection.destroy_bridge(mouse).await;
        }

        match result {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic)
        }
    }
}

impl Bridge {
    pub async fn execute(&self) -> Result<(), Error<'_>> {
        let connection = self.bus.connect().await?;
        let address = Self::parse_mac(self.remote_device.resolve()).await?;

        let bridges = bluekey::BridgeProxy::new(&connection).await?;
        let config = bluekey::ConfigProxy::new(&connection).await?;

        // Ensure bridges are destroyed on return
        Bridges::wrap(bridges.clone(),  async |devices| {
            let mut breakage_events = bridges.receive_bridge_broken().await?;
            let mut stdin = std::io::stdin().lock();

            // Create the bridges
            devices.mouse = match &self.targets.mouse {
                Some(mouse) => Some(bridges.bridge_mouse(mouse, &address.to_string()).await),
                None => None
            }.transpose()?;

            devices.keyboard = match &self.targets.keyboard {
                Some(keyboard) => {
                    println!("Press {} to break keyboard grab.", config.keyboard_escape_shortcut().await?);
                    Some(bridges.bridge_keyboard(keyboard, &address.to_string()).await)
                },
                None => None
            }.transpose()?;

            // Set up event loop to listen for when a keyboard or mouse bridge is destoryed
            let breakage_handle = async {
                while let Some(signal) = breakage_events.next().await {
                    let args = match signal.args() {
                        Ok(args) => args,
                        Err(error) => {
                            warn!("Invalid event from Bluekey; {error}");
                            continue
                        }
                    };

                    if devices.keyboard == Some(args.id) || devices.mouse == Some(args.id) {
                        return Error::BridgeDestroyed("Bridge broken by Bluekey")
                    }
                }
                Error::LostBluekey
            };

            // Wait for exit
            println!("Press enter to exit.");
            tokio::select! {
                _ = breakage_handle => (),
                _ = crate::wait_enter::async_wait_enter(&mut stdin) => ()
            };

            Ok(())
        }).await
    }

    async fn parse_mac<'a>(device: RemoteDevice<'a>) -> Result<Address, Error<'a>> {
        use super::ToBluerError;
        match device {
            RemoteDevice::Mac(address) => {
                match Address::from_str(address) {
                    Ok(address) => Ok(address),
                    Err(_) => Err(AddressError::AddressFormatting(address).into())
                }
            }
            RemoteDevice::Alias(alias) => {
                // Establish BlueZ connection
                let session = bluer::Session::new().await.connecting_error()?;
                let adapter = session.default_adapter().await.connecting_error()?;

                // Find the device
                Self::find_device(adapter, alias).await.into()
            },
        }
    }

    async fn find_device(adapter: Adapter, target_alias: &'_ str) -> Result<Address, Error<'_>> {
        use super::ToBluerError;
        let mut found = None;

        // Search all devices for the one specified by the alias
        for address in adapter.device_addresses().await.alias_error(target_alias)? {
            let alias = adapter.device(address).alias_error(target_alias)?.alias().await.alias_error(target_alias)?;

            if target_alias == alias {
                if found == None {
                    found = Some(address)
                } else {
                    return Err(AddressError::MultipleAliases(target_alias).into())
                }
            }
        };

        match found {
            None => Err(AddressError::NoSuchAlias(target_alias).into()),
            Some(address) => Ok(address)
        }

    }
}
