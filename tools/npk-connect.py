#!/usr/bin/env python3
"""npk-connect: Client for nopeekOS npk-shell

Usage: ./npk-connect.py [host] [port]
  host defaults to 127.0.0.1 (QEMU)
  port defaults to 4444

Requires: pip install cryptography
"""

import sys
import socket
import struct
import getpass
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDFExpand
from cryptography.hazmat.primitives import hashes, hmac as hmac_mod

HOST = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 4444


def hkdf_extract(salt: bytes, ikm: bytes) -> bytes:
    """HKDF-Extract (RFC 5869) with SHA-256."""
    h = hmac_mod.HMAC(salt, hashes.SHA256())
    h.update(ikm)
    return h.finalize()


def hkdf_expand_label(secret: bytes, label: bytes, context: bytes, length: int) -> bytes:
    """TLS 1.3-style HKDF-Expand-Label."""
    # HkdfLabel: length(2) + "tls13 " + label(1+N) + context(1+N)
    hkdf_label = struct.pack(">H", length)
    full_label = b"tls13 " + label
    hkdf_label += bytes([len(full_label)]) + full_label
    hkdf_label += bytes([len(context)]) + context

    # HKDF-Expand: T(1) = HMAC(PRK, info || 0x01)
    h = hmac_mod.HMAC(secret, hashes.SHA256())
    h.update(hkdf_label + b"\x01")
    expanded = h.finalize()
    return expanded[:length]


def derive_keys(shared_secret: bytes):
    salt = b"npk-shell-v1\x00" * 2 + b"\x00" * 6  # 32 bytes
    salt = b"npk-shell-v1" + b"\x00" * 20  # must match kernel: 32 bytes total
    prk = hkdf_extract(salt, shared_secret)

    s2c_key = hkdf_expand_label(prk, b"s2c key", b"", 32)
    c2s_key = hkdf_expand_label(prk, b"c2s key", b"", 32)
    s2c_iv = hkdf_expand_label(prk, b"s2c iv", b"", 12)
    c2s_iv = hkdf_expand_label(prk, b"c2s iv", b"", 12)
    return s2c_key, c2s_key, s2c_iv, c2s_iv


def build_nonce(iv: bytes, seq: int) -> bytes:
    nonce = bytearray(iv)
    seq_bytes = seq.to_bytes(8, "big")
    for i in range(8):
        nonce[4 + i] ^= seq_bytes[i]
    return bytes(nonce)


class NpkSession:
    def __init__(self, sock, send_key, recv_key, send_iv, recv_iv):
        self.sock = sock
        self.send_cipher = ChaCha20Poly1305(send_key)
        self.recv_cipher = ChaCha20Poly1305(recv_key)
        self.send_iv = send_iv
        self.recv_iv = recv_iv
        self.send_seq = 0
        self.recv_seq = 0

    def send(self, data: bytes):
        nonce = build_nonce(self.send_iv, self.send_seq)
        self.send_seq += 1
        encrypted = self.send_cipher.encrypt(nonce, data, None)
        frame = struct.pack(">H", len(encrypted)) + encrypted
        self.sock.sendall(frame)

    def recv(self) -> bytes:
        hdr = self._recv_exact(2)
        length = struct.unpack(">H", hdr)[0]
        data = self._recv_exact(length)
        nonce = build_nonce(self.recv_iv, self.recv_seq)
        self.recv_seq += 1
        return self.recv_cipher.decrypt(nonce, data, None)

    def send_str(self, s: str):
        self.send(s.encode())

    def recv_str(self) -> str:
        return self.recv().decode("utf-8", errors="replace")

    def _recv_exact(self, n: int) -> bytes:
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("connection closed")
            buf += chunk
        return buf


def main():
    print(f"[npk-connect] Connecting to {HOST}:{PORT}...")
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(10)
    sock.connect((HOST, PORT))
    print("[npk-connect] Connected. Key exchange...")

    # Receive server public key
    server_pub_bytes = b""
    while len(server_pub_bytes) < 32:
        server_pub_bytes += sock.recv(32 - len(server_pub_bytes))
    server_pub = X25519PublicKey.from_public_bytes(server_pub_bytes)

    # Generate client key pair and send public key
    client_priv = X25519PrivateKey.generate()
    client_pub = client_priv.public_key()
    sock.sendall(client_pub.public_bytes_raw())

    # Derive shared secret
    shared = client_priv.exchange(server_pub)
    s2c_key, c2s_key, s2c_iv, c2s_iv = derive_keys(shared)

    # Client sends with c2s keys, receives with s2c keys
    sess = NpkSession(sock, c2s_key, s2c_key, c2s_iv, s2c_iv)
    print("[npk-connect] Encrypted channel established.")

    # Auth handshake
    banner = sess.recv_str()
    if "PASSPHRASE?" not in banner:
        print(f"[npk-connect] Unexpected: {banner}")
        return

    passphrase = getpass.getpass("[npk-connect] Passphrase: ")
    sess.send_str(passphrase)

    response = sess.recv_str()
    if response != "OK":
        print(f"[npk-connect] Authentication failed: {response}")
        return

    print("[npk-connect] Authenticated. Type 'exit' to disconnect.\n")
    sock.settimeout(30)

    # Interactive loop
    try:
        while True:
            # Receive prompt
            prompt = sess.recv_str()
            # Get user input
            try:
                user_input = input(prompt)
            except (EOFError, KeyboardInterrupt):
                sess.send_str("exit")
                break

            sess.send_str(user_input)
            if user_input.strip() in ("exit", "quit", "disconnect"):
                try:
                    print(sess.recv_str())
                except Exception:
                    pass
                break

            # Receive output
            output = sess.recv_str()
            if output:
                print(output, end="")
    except (ConnectionError, BrokenPipeError):
        print("\n[npk-connect] Connection lost.")
    except Exception as e:
        print(f"\n[npk-connect] Error: {e}")
    finally:
        sock.close()
        print("[npk-connect] Disconnected.")


if __name__ == "__main__":
    main()
