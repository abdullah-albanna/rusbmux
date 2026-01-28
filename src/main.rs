// use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{
    fs,
    io::{Cursor, Read},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        fs::remove_file(socket_path).unwrap();
    }

    let listener = UnixListener::bind(socket_path).unwrap();

    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o666)).unwrap();

    loop {
        match listener.accept() {
            Ok((mut socket, _addr)) => {
                let mut raw_header_buf = vec![0; 16];
                socket.read_exact(&mut raw_header_buf).unwrap();

                let header: &rusbmux::UsbMuxHeader = bytemuck::from_bytes(&raw_header_buf);
                dbg!(&raw_header_buf);

                let payload_len = header.len.checked_sub(16).unwrap() as usize;

                dbg!(header);

                let mut buf = vec![0; payload_len];
                socket.read_exact(&mut buf[..]).unwrap();

                println!("raw payload: {buf:?}");

                println!("payload: \n{}", String::from_utf8_lossy(&buf));
            }
            Err(e) => println!("accept function failed: {e:?}"),
        }
    }
}
