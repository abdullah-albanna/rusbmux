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
                let mut buf = [0; 1024];

                let n = socket.read(&mut buf[..]).unwrap();

                println!("buf: {buf:?}, n: {n}");

                println!("{}", String::from_utf8_lossy(&buf[..n]));
            }
            Err(e) => println!("accept function failed: {e:?}"),
        }
    }
}
