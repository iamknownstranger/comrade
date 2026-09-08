#!/usr/bin/env python3
"""Static gate on the two promises the Capture tab makes that nothing else checks.

Capture is a camera app that can hand a shot to the phone's own camera app
(Google Camera on a Pixel) *and* a helmet cam that keeps recording once the
screen is locked. Both halves fail silently rather than loudly:

  * Drop `foregroundServiceType="camera|microphone"`, the WAKE_LOCK permission,
    or the ACTION_SCREEN_ON/OFF receiver, and everything still compiles, every
    unit test still passes, and the emulator lane still goes green — because
    none of them record anything. The failure shows up on a handset, an hour
    into a ride, as a file that stopped when the screen did.
  * Drop the `<queries>` declaration and `resolveActivity` returns null on API
    30+, so the "use my camera app" affordance quietly disappears on exactly
    the devices it was written for. Nothing fails; a feature is just not there.

`./gradlew test` cannot see any of this and neither can the two typecheck
scripts, so this file is the lane. It reads text, needs no SDK, and runs in
under a second.

Usage: python3 scripts/check_capture_contract.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "android/app/src/main/AndroidManifest.xml"
CAPTURE_DIR = ROOT / "android/app/src/main/java/mullu/comrade/capture"
CAMERA_APP = CAPTURE_DIR / "CameraApp.kt"
CAPTURE_SERVICE = CAPTURE_DIR / "CaptureService.kt"
CAPTURE_MANAGER = CAPTURE_DIR / "CaptureManager.kt"

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def read(path: Path) -> str:
    if not path.is_file():
        fail(f"{path.relative_to(ROOT)} does not exist")
        return ""
    return path.read_text(encoding="utf-8")


def check_screen_off_contract(manifest: str, service: str, manager: str) -> None:
    """The helmet cam's four load-bearing declarations."""
    service_decl = re.search(
        r"<service\b[^>]*android:name=\"\.capture\.CaptureService\"[^>]*/?>",
        manifest,
        re.S,
    )
    if service_decl is None:
        fail("AndroidManifest.xml does not declare .capture.CaptureService")
    else:
        types = re.search(r"android:foregroundServiceType=\"([^\"]+)\"", service_decl.group(0))
        declared = set(types.group(1).split("|")) if types else set()
        for required in ("camera", "microphone"):
            if required not in declared:
                fail(
                    "CaptureService's foregroundServiceType is missing "
                    f"'{required}' (declared: {sorted(declared) or 'none'}). Since "
                    "Android 11 a process with no visible Activity cannot open "
                    "the camera without it, so recording dies at the lock button."
                )

    for permission in (
        "android.permission.CAMERA",
        "android.permission.RECORD_AUDIO",
        "android.permission.WAKE_LOCK",
        "android.permission.FOREGROUND_SERVICE",
        "android.permission.FOREGROUND_SERVICE_CAMERA",
        "android.permission.FOREGROUND_SERVICE_MICROPHONE",
    ):
        if f'android:name="{permission}"' not in manifest:
            fail(f"AndroidManifest.xml no longer declares {permission}")

    for action in ("ACTION_SCREEN_OFF", "ACTION_SCREEN_ON"):
        if f"Intent.{action}" not in service:
            fail(
                f"CaptureService.kt no longer handles Intent.{action}. These "
                "cannot be declared in the manifest, so this dynamic receiver is "
                "the only thing that tells CaptureManager to drop the preview — "
                "without it a locked phone keeps compositing a viewfinder nobody "
                "is looking at."
            )

    if "PARTIAL_WAKE_LOCK" not in manager:
        fail(
            "CaptureManager.kt no longer takes a PARTIAL_WAKE_LOCK. A foreground "
            "service does not by itself keep the CPU out of suspend."
        )


def check_handoff_gate(manifest: str, camera_app: str) -> None:
    """Package visibility for the handoff, and one gate in front of it."""
    actions = dict(
        re.findall(
            r"const val (ACTION_[A-Z_]+):\s*String\s*=\s*\"([^\"]+)\"",
            camera_app,
        )
    )
    if not actions:
        fail(
            "CameraApp.kt declares no ACTION_* handoff constants — this gate "
            "cannot tell what the manifest needs to be able to see."
        )
        return

    queries = re.search(r"<queries>(.*?)</queries>", manifest, re.S)
    visible = set(re.findall(r"<action\s+android:name=\"([^\"]+)\"", queries.group(1))) if queries else set()
    for name, value in sorted(actions.items()):
        if value not in visible:
            fail(
                f"AndroidManifest.xml's <queries> does not declare {value} "
                f"(CameraApp.{name}). Without it, resolveActivity/queryIntentActivities "
                "answer null on API 30+ and the handoff to the phone's own camera "
                "app disappears with no error anywhere."
            )

    # One gate, not several: CameraApp.handoff is where the "a ride never leaves
    # this app" rule is enforced and unit-tested. A second place that builds the
    # same intent from a literal would bypass it without failing any test.
    for path in sorted(ROOT.glob("android/app/src/main/java/**/*.kt")):
        if path == CAMERA_APP:
            continue
        text = path.read_text(encoding="utf-8")
        for name, value in actions.items():
            if f'"{value}"' in text:
                fail(
                    f"{path.relative_to(ROOT)} spells out \"{value}\" as a literal. "
                    f"Use CameraApp.{name} so the handoff decision (and the rule "
                    "that a screen-off recording is never delegated) stays in one "
                    "tested place."
                )


def main() -> int:
    manifest = read(MANIFEST)
    service = read(CAPTURE_SERVICE)
    manager = read(CAPTURE_MANAGER)
    camera_app = read(CAMERA_APP)
    if not failures:
        check_screen_off_contract(manifest, service, manager)
        check_handoff_gate(manifest, camera_app)

    if failures:
        for message in failures:
            print(f"::error::{message}", file=sys.stderr)
        print(f"\n{len(failures)} capture-contract check(s) failed.", file=sys.stderr)
        return 1
    print("capture contract OK: screen-off recording declarations intact, handoff visible and gated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
