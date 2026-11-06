"""The acceptance sender at the network seam; no microphone, decoder, or sound device."""
import importlib.util
from pathlib import Path
import socket
import struct
import threading
import unittest


class SenderTests(unittest.TestCase):
    def test_untrusted_response_sizes_are_rejected_before_reading_the_body(self):
        spec = importlib.util.spec_from_file_location('sender', Path(__file__).with_name('audio_backchannel.py'))
        sender = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(sender)
        for response in [
            b'RTSP/1.0 200 OK\r\nContent-Length: 999999999\r\n\r\n',
            b'RTSP/1.0 200 OK\r\nX: ' + b'x' * 16384 + b'\r\n\r\n',
            b'RTSP/1.0 200 OK\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n',
        ]:
            with self.subTest(response=response[:80]), socket.socket() as listener:
                listener.bind(('127.0.0.1', 0))
                listener.listen()
                listener.settimeout(2)

                def camera():
                    sock, _ = listener.accept()
                    with sock, sock.makefile('rb') as stream:
                        sock.settimeout(2)
                        while stream.readline() != b'\r\n':
                            pass
                        sock.sendall(response)

                thread = threading.Thread(target=camera)
                thread.start()
                try:
                    with self.assertRaisesRegex(ValueError, 'RTSP response'):
                        sender.send_opus('127.0.0.1', listener.getsockname()[1], 'Preview_01_sub', [b'\xf8'])
                finally:
                    thread.join(timeout=3)
                self.assertFalse(thread.is_alive())

    def test_existing_opus_packets_use_onvif_setup_play_and_paced_rtp(self):
        spec = importlib.util.spec_from_file_location('sender', Path(__file__).with_name('audio_backchannel.py'))
        sender = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(sender)
        requests, packets, errors = [], [], []
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            listener.settimeout(3)
            port = listener.getsockname()[1]

            def camera():
                try:
                    sock, _ = listener.accept()
                    with sock, sock.makefile('rb') as stream:
                        sock.settimeout(3)
                        for _ in range(3):
                            lines = []
                            while (line := stream.readline()) != b'\r\n':
                                if not line:
                                    raise EOFError()
                                lines.append(line)
                            requests.append(b''.join(lines))
                            sock.sendall(b'RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: abc\r\nContent-Length: 0\r\n\r\n')
                        for _ in range(2):
                            hdr = stream.read(4)
                            self.assertEqual(hdr[:2], b'$\x04')
                            packets.append(stream.read(struct.unpack('!H', hdr[2:])[0]))
                        requests.append(stream.readline())
                except BaseException as exc:
                    errors.append(exc)

            thread = threading.Thread(target=camera)
            thread.start()
            # Synthetic bytes are confined to this host-only network test: two 20 ms CELT TOC bytes.
            payload = [b'\xf8\x01\x02', b'\xf8\x03']
            result = sender.send_opus('127.0.0.1', port, 'Preview_01_sub', payload)
            thread.join(timeout=4)
            self.assertFalse(thread.is_alive())
            self.assertEqual(errors, [])
        self.assertEqual(result, 2)
        self.assertIn(b'Require: www.onvif.org/ver20/backchannel', requests[0])
        self.assertIn(b'/trackID=2 RTSP/1.0', requests[1])
        self.assertTrue(requests[2].startswith(b'PLAY '))
        self.assertTrue(requests[3].startswith(b'TEARDOWN '))
        self.assertEqual([packets[0][12:], packets[1][12:]], payload)
        self.assertEqual(struct.unpack('!BBHI', packets[0][:8]), (0x80, 111, 0, 0))
        self.assertEqual(struct.unpack('!BBHI', packets[1][:8]), (0x80, 111, 1, 960))

    def test_ogg_opus_files_yield_audio_packets_with_toc_durations(self):
        spec = importlib.util.spec_from_file_location('sender', Path(__file__).with_name('audio_backchannel.py'))
        sender = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(sender)

        def page(*packets):
            lacing, body = b'', b''
            for packet in packets:
                lacing += bytes([255] * (len(packet) // 255) + [len(packet) % 255])
                body += packet
            return b'OggS' + bytes(22) + bytes([len(lacing)]) + lacing + body

        long = b'\xf8' + bytes(299)  # spans two lacing values
        stream = page(b'OpusHead' + bytes(11)) + page(b'OpusTags' + bytes(8)) + page(b'\xf8\x01', long)
        self.assertEqual(sender.ogg_opus_packets(stream), [b'\xf8\x01', long])
        with self.assertRaisesRegex(ValueError, 'Ogg Opus'):
            sender.ogg_opus_packets(page(b'vorbis'))
        self.assertEqual(sender.duration(b'\xf8'), 960)  # CELT 20 ms
        self.assertEqual(sender.duration(b'\x08'), 960)  # SILK 20 ms
        self.assertEqual(sender.duration(b'\x09'), 1920)  # code 1, two frames
        self.assertEqual(sender.duration(b'\xfb\x03'), 2880)  # code 3, three 20 ms frames


if __name__ == '__main__':
    unittest.main()
