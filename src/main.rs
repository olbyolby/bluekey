// A very awful, but technically functional, Bluetooth keyboard emulator(and an unused battery emulator)
use std::sync::{Arc, atomic::AtomicBool};
use tokio::io::{AsyncBufReadExt, BufReader};
use evdev::{Device, EventSummary};

mod bluetooth;


#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
    //main2().await;

    println!("Creating keyboard interface");
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    let board = bluetooth::keyboard::start_keyboard(adapter).await?;

    println!("Press enter to quit");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    
    println!("Pulling events");
    let mut device = Device::open("/dev/input/by-id/usb-_RPI_Wired_Keyboard_4-event-kbd").unwrap();
    device.grab().unwrap();
    
    let run_loop = Arc::new(AtomicBool::new(true));
    let run_loop_flag = run_loop.clone();
    std::thread::spawn(move || {
        while run_loop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            for event in device.fetch_events().unwrap() {
                if let EventSummary::Key(_, code, action) = event.destructure() {
                    if let Ok(map) = keycode::KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.0)) {
                        let code: u8 = map.usb.try_into().unwrap();
                        match action {
                            0 => board.try_release(code).unwrap(),
                            1 => board.try_press(code).unwrap(),
                            _ => println!("Invalid action: {:?}", action)
                        };
                    } else {
                        println!("Invalid key: {:?}", code);
                    }
                }
                
            }
        }
    });

    loop {
        tokio::select! {
            _ = lines.next_line() => break 
            
        }
    }

    Ok(())

    
}
