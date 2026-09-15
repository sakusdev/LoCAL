"""Expose the generated dependency lock to a build client without Git credentials."""
import base64
import pathlib
import zlib

lock = pathlib.Path("Cargo.lock")
if lock.exists():
    print("LOCAL_LOCK_ZLIB=" + base64.b64encode(zlib.compress(lock.read_bytes())).decode())

