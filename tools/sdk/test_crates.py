"""Tests for the package patches in crates.py: python3 -m unittest discover tools/sdk."""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import crates  # noqa: E402


def sample_crate() -> tuple[dict[str, bytes], dict[str, bytes], bytes]:
    lib = bytes(range(256)) * 40 + b"!<arch>\n"
    return {"build.rs": crates.WINDOWS_BUILD.encode()}, {"lib/windows.0.52.0.lib": lib}, lib


class WindowsMsvcPatch(unittest.TestCase):
    def test_patched_names(self):
        for name in ("windows_aarch64_msvc", "windows_x86_64_msvc", "windows_i686_msvc"):
            self.assertIs(crates.PATCHES[name], crates.patch_windows_msvc)

    def test_hex_text_round_trips_through_the_build_script(self):
        rustc = shutil.which("rustc")
        if not rustc:
            self.skipTest("no rustc")
        kept, refused, lib = sample_crate()
        crates.patch_windows_msvc("windows_x86_64_msvc-0.52.6", kept, refused)
        self.assertEqual(refused, {})
        hex_lines = kept["lib/windows.0.52.0.lib.hex"].decode().split("\n")
        self.assertTrue(all(len(line) <= 128 for line in hex_lines))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "pkg" / "lib").mkdir(parents=True)
            (root / "pkg" / "lib" / "windows.0.52.0.lib.hex").write_bytes(kept["lib/windows.0.52.0.lib.hex"])
            (root / "out").mkdir()
            (root / "build.rs").write_bytes(kept["build.rs"])
            exe = root / ("build" + (".exe" if os.name == "nt" else ""))
            subprocess.run([rustc, "--edition", "2021", "-o", str(exe), str(root / "build.rs")], check=True)
            env = {**os.environ, "CARGO_MANIFEST_DIR": str(root / "pkg"), "OUT_DIR": str(root / "out")}
            said = subprocess.run([str(exe)], env=env, check=True, capture_output=True, text=True).stdout
            self.assertEqual((root / "out" / "windows.0.52.0.lib").read_bytes(), lib)
            self.assertIn(f"cargo:rustc-link-search=native={root / 'out'}", said)

    def test_refuses_a_build_script_of_another_shape(self):
        kept, refused, _ = sample_crate()
        kept["build.rs"] = b'fn main() { println!("cargo:rustc-link-lib=windows"); }\n'
        with self.assertRaises(crates.Problem):
            crates.patch_windows_msvc("windows_x86_64_msvc-0.52.6", kept, refused)

    def test_a_crate_with_no_refused_library_is_left_alone(self):
        kept, _, _ = sample_crate()
        before = dict(kept)
        crates.patch_windows_msvc("windows_x86_64_msvc-0.52.6", kept, {})
        self.assertEqual(kept, before)


if __name__ == "__main__":
    unittest.main()
