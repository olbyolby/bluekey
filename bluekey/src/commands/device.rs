use std::str::FromStr;

use bluer::{Adapter, Address};
use clap::Args;

use super::{AddressError, Error, ToBluerError};

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct RemoteDeviceArgs {
    #[arg(long)]
    /// MAC address of the target device
    pub mac: Option<String>,
    #[arg(long)]
    /// Name/alias of the target device
    pub alias: Option<String>
}
impl RemoteDeviceArgs {
    pub fn resolve(&self) -> RemoteDevice<'_> {
        if let Some(mac) = &self.mac {
            return RemoteDevice::Mac(mac)
        } else if let Some(alias) = &self.alias {
            return RemoteDevice::Alias(alias)
        }
        unreachable!("Clap should not allow more than 2 of these arguments")
    }

    pub async fn parse_mac(&self) -> Result<Address, Error<'_>> {
        match self.resolve() {
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

#[derive(Clone, Copy)]
pub enum RemoteDevice<'a> {
    Mac(&'a str),
    Alias(&'a str)
}
