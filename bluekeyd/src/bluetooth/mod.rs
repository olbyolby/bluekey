use std::{collections::{HashMap, hash_map::Entry}, fmt::Debug};

use bluer::Address;
use tokio::sync::{mpsc, broadcast};

pub mod keyboard;
pub mod mouse;
pub mod leds;
mod hid;

#[derive(Clone, Copy, Debug)]
pub enum Target {
    Target(Address),
    Broadcast
}

struct Register(Address);
struct DeviceMap<T, E> {
    devices: HashMap<Address, T>,
    return_events: broadcast::Sender<E>
}
enum TargetIter<A:, B> {
    Broadcast(A),
    Target(Option<B>), 
}
impl<'a, T: 'a, A: Iterator<Item=&'a mut T>, B: Iterator<Item=&'a mut T>> Iterator for TargetIter<A, B> {
    type Item =  &'a mut T;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            TargetIter::Broadcast(iter) => iter.next(),
            TargetIter::Target(Some(iter)) => iter.next(),
            TargetIter::Target(None) => None
        }
    }
}

impl<T: Default, E: From<Register> + Debug> DeviceMap<T, E> {
    fn new(return_events: broadcast::Sender<E>) -> Self {
        DeviceMap {
            devices: HashMap::new(),
            return_events: return_events
        }
    }

    async fn acquire_device(&mut self, address: Address) -> &mut T {
        match self.devices.entry(address) {
            Entry::Occupied(device) => device.into_mut(),
            Entry::Vacant(device) => {
                // So far as I can tell, there is no good way to detect disconnects, so this *will* slowly leak memory, unfortunately
                let device = device.insert(Default::default());
                self.return_events.send(Register(address).into()).unwrap();
                device
            }
        }
    }
    fn get_device_mut(&mut self, address: Address) -> Option<&mut T> {
        self.devices.get_mut(&address)
    }
    fn get_device(&self, address: Address) -> Option<&T> {
        self.devices.get(&address)
    }

    fn get_targets(&mut self, target: Target) -> impl Iterator<Item=&mut T> {
        match target {
            Target::Broadcast => TargetIter::Broadcast(self.devices.values_mut()),
            Target::Target(target) => TargetIter::Target(self.get_device_mut(target).map(|device| std::iter::once(device)))
        }
    }
}

enum ReturnError {
    ServerDied,
    Lagged(u64)
}
pub struct ReturnEventListener<T> {
    receiver: broadcast::Receiver<T>   
}
impl<T: Copy> ReturnEventListener<T> {
    pub async fn next_event(&mut self) -> Result<T, ReturnError> {
        match self.receiver.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Closed) => Err(ReturnError::ServerDied),
            Err(broadcast::error::RecvError::Lagged(lag)) => Err(ReturnError::Lagged(lag))
        }
    }
}