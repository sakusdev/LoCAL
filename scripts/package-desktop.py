"""Collect deterministic, distinctly named packages for a Release matrix row."""
import argparse
import pathlib
import shutil
import tarfile
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument("target")
parser.add_argument("platform")
args = parser.parse_args()
version = "0.4.1"
root = pathlib.Path(__file__).resolve().parents[1]
build = root / "target" / args.target / "release"
out = root / "dist"
out.mkdir(exist_ok=True)
base = f"LoCAL-{version}-{args.platform}"
cli = build / ("local.exe" if args.platform.startswith("windows") else "local")
assert cli.is_file(), cli
if args.platform.startswith("windows"):
    with zipfile.ZipFile(out / f"{base}-portable.zip", "w", zipfile.ZIP_DEFLATED) as archive:
        for path in [build / "LoCAL.exe", cli, root / "README.md", root / "docs" / "QUICKSTART.md", root / "LICENSE"]:
            archive.write(path, path.name)
else:
    with tarfile.open(out / f"{base}-cli.tar.gz", "w:gz") as archive:
        for path in [cli, root / "README.md", root / "docs" / "QUICKSTART.md", root / "LICENSE"]:
            archive.add(path, arcname=path.name)
extensions = {".exe":"-setup.exe", ".dmg":".dmg", ".deb":".deb", ".AppImage":".AppImage"}
found = 0
for path in (build / "bundle").rglob("*"):
    if path.is_file() and path.suffix in extensions:
        if path.suffix == ".exe" and path.parent.name != "nsis":
            continue
        destination = out / (base + extensions[path.suffix])
        if destination.exists():
            raise RuntimeError(f"Duplicate package: {destination}")
        shutil.copy2(path, destination)
        found += 1
assert found, "No GUI bundles produced"
print("\n".join(str(path) for path in out.iterdir()))
