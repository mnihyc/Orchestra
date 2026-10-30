#!/usr/bin/env python3
"""A VNC client small enough to ask a display's server what it is and what it shows.

It speaks the protocol's own handshake (version 3.8, whatever way of asking who is
there the server offers, the screen's size and format), can hold a connection while
another is made, and can take one picture of the screen in the plainest encoding. It
needs nothing but the standard library, because it runs inside the image it checks.

    rfb.py info  PORT            print the server's name, size, depth and the ways it asks who is there
    rfb.py shot  PORT FILE.png   the same, and write what the screen shows as a PNG
    rfb.py share PORT            two clients, the second asking for the screen to itself; says
                                 whether the first is still served afterwards

Every line it prints is `key=value`.
"""
import socket
import struct
import sys
import zlib

HOST = "127.0.0.1"
# The protocol's version this client speaks; every server of the last fifteen years offers it.
VERSION = b"RFB 003.008\n"
# What a read may wait: a loaded machine is slow, a server that does not answer in this time is not there.
TIMEOUT_SECONDS = 20
# Security types of the protocol, by number.
SECURITY_NONE = 1
SECURITY_NAMES = {1: "None", 2: "VNC", 16: "Tight", 19: "VeNCrypt", 30: "ARD", 129: "TightVNC-unix-login"}
# Encoding 0 is raw pixels, which every server has.
ENCODING_RAW = 0


class Client:
    """One connection, taken as far as the screen's description."""

    def __init__(self, port, shared=0):
        self.sock = socket.create_connection((HOST, port), timeout=TIMEOUT_SECONDS)
        banner = self.read(12)
        if not banner.startswith(b"RFB "):
            raise RuntimeError("not a VNC server: it said %r" % banner)
        self.server_version = banner.decode("ascii", "replace").strip()
        self.sock.sendall(VERSION)
        count = self.read(1)[0]
        if count == 0:
            length = struct.unpack(">I", self.read(4))[0]
            raise RuntimeError("the server refused: %s" % self.read(length).decode("utf-8", "replace"))
        self.security = list(self.read(count))
        if SECURITY_NONE not in self.security:
            raise RuntimeError("the server does not offer to take nobody: it offers %r" % self.security)
        self.sock.sendall(bytes([SECURITY_NONE]))
        result = struct.unpack(">I", self.read(4))[0]
        if result != 0:
            raise RuntimeError("the server did not accept: security result %d" % result)
        # ClientInit: 1 asks to share the screen with whoever is there, 0 asks to have it alone.
        self.sock.sendall(bytes([shared]))
        header = self.read(24)
        self.width, self.height = struct.unpack(">HH", header[:4])
        (self.bits_per_pixel, self.depth, self.big_endian, self.true_colour,
         self.red_max, self.green_max, self.blue_max,
         self.red_shift, self.green_shift, self.blue_shift) = struct.unpack(">BBBBHHHBBB3x", header[4:20])
        name_length = struct.unpack(">I", header[20:24])[0]
        self.name = self.read(name_length).decode("utf-8", "replace")

    def read(self, count):
        data = b""
        while len(data) < count:
            chunk = self.sock.recv(count - len(data))
            if not chunk:
                raise RuntimeError("the connection was closed")
            data += chunk
        return data

    def read_update(self, pixels=None):
        """Reads what the server sends until one whole picture update has been read; how many pixels it covered.

        With a buffer given, the pixels are put into it where the update says they belong.
        """
        bytes_per_pixel = self.bits_per_pixel // 8
        while True:
            kind = self.read(1)[0]
            if kind == 0:
                rectangles = struct.unpack(">xH", self.read(3))[0]
                covered = 0
                for _ in range(rectangles):
                    x, y, w, h, encoding = struct.unpack(">HHHHi", self.read(12))
                    if encoding != ENCODING_RAW:
                        raise RuntimeError("the server sent encoding %d, which was not asked for" % encoding)
                    data = self.read(w * h * bytes_per_pixel)
                    if pixels is not None:
                        for row in range(h):
                            start = ((y + row) * self.width + x) * bytes_per_pixel
                            pixels[start:start + w * bytes_per_pixel] = data[row * w * bytes_per_pixel:(row + 1) * w * bytes_per_pixel]
                    covered += w * h
                return covered
            if kind == 1:  # SetColourMapEntries
                _, _, count = struct.unpack(">BHH", self.read(5))
                self.read(count * 6)
            elif kind == 2:  # Bell
                pass
            elif kind == 3:  # ServerCutText: what was copied in the display
                length = struct.unpack(">xxxI", self.read(7))[0]
                self.read(length)
            else:
                raise RuntimeError("a message of type %d, which this client does not know" % kind)

    def ask_for_pixels(self):
        """One full picture of the screen, as raw pixels in the server's own format."""
        # SetEncodings: raw only. FramebufferUpdateRequest: not incremental, the whole screen.
        self.sock.sendall(struct.pack(">BxHi", 2, 1, ENCODING_RAW))
        self.sock.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, self.width, self.height))
        pixels = bytearray(self.width * self.height * (self.bits_per_pixel // 8))
        covered = 0
        while covered < self.width * self.height:
            covered += self.read_update(pixels)
        return bytes(pixels)

    def alive(self):
        """Whether the server still serves this client: it is asked for the screen's smallest corner."""
        try:
            self.sock.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, 1, 1))
            self.read_update()
            return True
        except (OSError, RuntimeError):
            return False

    def close(self):
        self.sock.close()


def rgb_of(client, pixels):
    """The picture as 8-bit red, green, blue for each pixel, when the server's format is the usual one."""
    if (client.bits_per_pixel, client.big_endian, client.true_colour) != (32, 0, 1):
        return None
    if (client.red_max, client.green_max, client.blue_max) != (255, 255, 255):
        return None
    channel = {client.red_shift // 8: 0, client.green_shift // 8: 1, client.blue_shift // 8: 2}
    rgb = bytearray(client.width * client.height * 3)
    for byte_index, rgb_index in channel.items():
        rgb[rgb_index::3] = pixels[byte_index::4]
    return bytes(rgb)


def png_of(width, height, rgb):
    """A PNG of the picture, written with nothing but zlib."""
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    rows = b"".join(b"\x00" + rgb[y * width * 3:(y + 1) * width * 3] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b""))


def describe(client):
    print("version=%s" % client.server_version)
    print("security=%s" % ",".join(SECURITY_NAMES.get(t, str(t)) for t in client.security))
    print("name=%s" % client.name)
    print("size=%dx%d" % (client.width, client.height))
    print("depth=%d" % client.depth)
    print("bits_per_pixel=%d" % client.bits_per_pixel)


def main(argv):
    if len(argv) < 3 or argv[1] not in ("info", "shot", "share"):
        sys.stderr.write(__doc__)
        return 2
    port = int(argv[2])
    if argv[1] == "info":
        client = Client(port)
        describe(client)
        client.close()
    elif argv[1] == "shot":
        client = Client(port)
        describe(client)
        pixels = client.ask_for_pixels()
        rgb = rgb_of(client, pixels)
        if rgb is None:
            print("picture=unreadable (the server's pixel format is not the usual one)")
        else:
            colours = len(set(zip(rgb[0::3], rgb[1::3], rgb[2::3])))
            print("colours=%d" % colours)
            if len(argv) > 3:
                with open(argv[3], "wb") as handle:
                    handle.write(png_of(client.width, client.height, rgb))
                print("written=%s" % argv[3])
        client.close()
    else:
        first = Client(port, shared=0)
        before = first.alive()
        second = Client(port, shared=0)
        print("first_served_before=%s" % ("yes" if before else "no"))
        print("second_connected=yes")
        print("first_served_after=%s" % ("yes" if first.alive() else "no"))
        print("second_served=%s" % ("yes" if second.alive() else "no"))
        first.close()
        second.close()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except (OSError, RuntimeError) as error:
        print("error=%s" % error)
        sys.exit(1)
