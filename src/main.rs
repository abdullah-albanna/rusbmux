use std::{
    fs,
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
};

use rusbmux::types::UsbMuxHeader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        fs::remove_file(socket_path).unwrap();
    }

    let listener = UnixListener::bind(socket_path).unwrap();

    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o666)).unwrap();

    loop {
        match listener.accept() {
            Ok((mut socket, _addr)) => {
                let mut raw_header_buf = [0; UsbMuxHeader::SIZE];
                socket.read_exact(&mut raw_header_buf).unwrap();

                let header: &UsbMuxHeader = bytemuck::from_bytes(&raw_header_buf);

                let payload_len = header.len.checked_sub(16).unwrap() as usize;

                let mut buf = vec![0; payload_len];
                socket.read_exact(&mut buf[..]).unwrap();

                println!("payload: \n{}", String::from_utf8_lossy(&buf));

                let p = plist::from_bytes::<plist::Dictionary>(&buf).unwrap();

                if p.get("MessageType").unwrap().as_string().unwrap() == "ListDevices" {
                    rusbmux::send_device_list(&mut socket, header.tag).await;
                }
            }
            Err(e) => println!("accept function failed: {e:?}"),
        }
    }
}
