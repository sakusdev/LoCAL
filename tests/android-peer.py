"""Linux peer for the Android emulator interoperability test, on a fresh identity."""
import json
import pathlib
import subprocess
import sys
import tempfile
import time

root = pathlib.Path(tempfile.mkdtemp(prefix="local-android-test-"))
payload = bytes(n % 251 for n in range(2_500_007))
source = root / "from-linux.bin"
source.write_bytes(payload)
process = subprocess.Popen([sys.argv[1], "--json", "--name", "Linux CI", "--no-discovery", "--data-dir", str(root / "data"), "--receive-dir", str(root / "received")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

def command(**data):
    process.stdin.write(json.dumps(data) + "\n")
    process.stdin.flush()
    line = process.stdout.readline()
    if not line:
        raise RuntimeError("CLI stopped")
    response = json.loads(line)
    if not response["ok"]:
        raise RuntimeError(response["error"])
    return response["data"]

sent = False
try:
    while True:
        state = command(op="snapshot")
        for peer in state["peers"]:
            if peer["code"] and not peer["local_confirmed"]:
                command(op="confirm", peer_id=peer["id"], code=peer["code"])
            if peer["ready"] and not sent:
                command(op="send_text", peer_id=peer["id"], text="Hello Android from Linux 👋")
                command(op="send_file", peer_id=peer["id"], path=str(source))
                sent = True
        for transfer in state["transfers"]:
            if transfer["status"] == "offered":
                command(op="accept_file", id=transfer["id"], accept=True)
            if transfer["direction"] == "in" and transfer["status"] == "completed":
                assert pathlib.Path(transfer["path"]).read_bytes() == payload
                print("PASS Android -> Linux 2.5MB content verified", flush=True)
                pathlib.Path("android-host-passed").write_text("passed")
                # Stay alive until the emulator job finishes.
                while process.poll() is None:
                    time.sleep(1)
        time.sleep(0.05)
finally:
    process.terminate()

