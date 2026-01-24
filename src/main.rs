use std::{
    fs,
    io::Read,
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
                let mut header_buf = [0; 16];
                socket.read_exact(&mut header_buf).unwrap();

                println!("raw payload header: {header_buf:?}");

                let total_len = u32::from_le_bytes(header_buf[..4].try_into().unwrap()) as usize;
                let payload_len = total_len.checked_sub(16).unwrap();

                let version = u32::from_le_bytes(header_buf[4..8].try_into().unwrap());
                let message_type = u32::from_le_bytes(header_buf[8..12].try_into().unwrap());
                let tag = u32::from_le_bytes(header_buf[12..16].try_into().unwrap());

                dbg!(total_len);
                dbg!(payload_len);
                dbg!(version);
                dbg!(message_type);
                dbg!(tag);

                let mut buf = vec![0; payload_len];
                socket.read_exact(&mut buf[..]).unwrap();

                println!("raw payload: {buf:?}");

                println!("payload: \n{}", String::from_utf8_lossy(&buf));
            }
            Err(e) => println!("accept function failed: {e:?}"),
        }
    }
}
