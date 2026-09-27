import pathlib
import platform
import subprocess

subprocess.run(
    ["cargo", "about", "generate", "about.hbs", "-o", "THIRD_PARTY_LICENSES"],
    check=True,
)

if platform.system() == "Windows":
    subprocess.run(
        ["cargo", "about", "generate", "about.hbs", "-o", "THIRD_PARTY_LICENSES_USB"],
        check=True,
    )
    with open("THIRD_PARTY_LICENSES", "a+") as file:
        usb_licenses = pathlib.Path("THIRD_PARTY_LICENSES_USB").read_text()
        libusb_license = pathlib.Path("windows/LICENSE-LIBUSB.txt").read_text()
        libusb_win32_license = pathlib.Path(
            "windows/LICENSE-LIBUSB-WIN32.txt"
        ).read_text()

        file.write(
            "`n`n----------------------------------------------------------------"
        )
        file.write(
            "-------------------- Windows USB Driver Dependencies --------------------"
        )
        file.write("---------------------------------------------------------------`n")
        file.write(usb_licenses)

        file.write(f"""
GNU Lesser General Public License v2.1
- libusb (rusb backend)
--------------------------------------------------------------------------------
{libusb_license}


GNU GENERAL PUBLIC LICENSE Version 3
- libusb-win32 (driver)
--------------------------------------------------------------------------------
{libusb_win32_license}
        """)
