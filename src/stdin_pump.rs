//! Raw stdin reader: move host-terminal input bytes into the app event loop.

use std::io::{self, Read};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use crate::app::HostEvent;

pub fn spawn(tx: mpsc::Sender<HostEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut buf = [0u8; 4096];

        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(HostEvent::Bytes(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}
