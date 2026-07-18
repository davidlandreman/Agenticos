import socket
import threading
import unittest

from tools import clipboard_bridge


class FakeClipboard:
    def __init__(self, text=b""):
        self.text = text

    def copy(self, text):
        self.text = text

    def paste(self):
        return self.text


class ClipboardBridgeTests(unittest.TestCase):
    def test_copy_and_paste_round_trip(self):
        clipboard = FakeClipboard()
        self.assertEqual(
            clipboard_bridge.handle_request(clipboard_bridge.OP_COPY, "hello ☃".encode(), clipboard),
            b"",
        )
        self.assertEqual(
            clipboard_bridge.handle_request(clipboard_bridge.OP_PASTE, b"", clipboard),
            "hello ☃".encode(),
        )

    def test_binary_input_is_rejected(self):
        with self.assertRaises(clipboard_bridge.BridgeError) as caught:
            clipboard_bridge.handle_request(
                clipboard_bridge.OP_COPY, b"\xff", FakeClipboard()
            )
        self.assertEqual(caught.exception.status, clipboard_bridge.STATUS_INVALID_TEXT)

    def test_framed_connection(self):
        guest, host = socket.socketpair()
        clipboard = FakeClipboard(b"host text")

        def serve_one():
            try:
                raw = clipboard_bridge.read_exact(host, clipboard_bridge.HEADER.size)
                magic, version, operation, length = clipboard_bridge.HEADER.unpack(raw)
                payload = clipboard_bridge.read_exact(host, length)
                response = clipboard_bridge.handle_request(operation, payload, clipboard)
                clipboard_bridge.send_response(host, clipboard_bridge.STATUS_OK, response)
                self.assertEqual(magic, clipboard_bridge.REQUEST_MAGIC)
                self.assertEqual(version, clipboard_bridge.VERSION)
            finally:
                host.close()

        thread = threading.Thread(target=serve_one)
        thread.start()
        guest.sendall(
            clipboard_bridge.HEADER.pack(
                clipboard_bridge.REQUEST_MAGIC,
                clipboard_bridge.VERSION,
                clipboard_bridge.OP_PASTE,
                0,
            )
        )
        header = clipboard_bridge.read_exact(guest, clipboard_bridge.HEADER.size)
        magic, version, status, length = clipboard_bridge.HEADER.unpack(header)
        self.assertEqual((magic, version, status), (b"ACBR", 1, 0))
        self.assertEqual(clipboard_bridge.read_exact(guest, length), b"host text")
        guest.close()
        thread.join()


if __name__ == "__main__":
    unittest.main()
