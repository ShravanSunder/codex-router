#!/usr/bin/env python3
"""External code fixture only; not the Router CLI or release identity."""
import json
import mmap
import os
import signal
import sys
import time
from pathlib import Path

FIXTURE_MODE = "valid"
CODE_MARKER = "IMAGE_A"
PROOF_PATH = None
BUILD_INFO = {"packageVersion": "1.2.3", "fingerprints": {
    "keeper": "1111111111111111111111111111111111111111111111111111111111111111",
    "agentCollaborationServices": "1111111111111111111111111111111111111111111111111111111111111111",
    "agentProxyServices": "1111111111111111111111111111111111111111111111111111111111111111",
    "agentProviderServices": "1111111111111111111111111111111111111111111111111111111111111111"}}

if sys.argv[1:] == ["build-info", "--json"]:
    if FIXTURE_MODE == "vm-teardown":
        # One bounded external fixture: retain 64 MiB of anonymous, touched VM
        # until normal os._exit(0), rather than manufacturing any syscall error.
        retained_mapping = mmap.mmap(-1, 64 * 1024 * 1024)
        for page_offset in range(0, 64 * 1024 * 1024, 4096):
            retained_mapping[page_offset] = 1
    if PROOF_PATH is not None:
        Path(PROOF_PATH).write_text(str(os.getpid()))
    if FIXTURE_MODE == "timeout":
        time.sleep(30)
    elif FIXTURE_MODE == "invalid":
        print("{")
    elif FIXTURE_MODE == "oversized":
        sys.stdout.write("X" * (1024 * 1024 + 2))
    elif FIXTURE_MODE == "trailing":
        print(json.dumps(BUILD_INFO) + " trailing-invalid-data")
    else:
        if FIXTURE_MODE == "version":
            BUILD_INFO["packageVersion"] = "1.2.4"
        if FIXTURE_MODE == "tamper":
            with open(sys.argv[0], "a") as image:
                image.write("\n# changed copied image after execution\n")
        if FIXTURE_MODE == "mismatch":
            BUILD_INFO["fingerprints"]["agentProviderServices"] = "33" * 32
        print(json.dumps(BUILD_INFO))
        if FIXTURE_MODE == "nonzero":
            sys.exit(3)
    if FIXTURE_MODE == "vm-teardown":
        sys.stdout.flush()
        os.close(1)
        os._exit(0)
    if FIXTURE_MODE in {"mismatch", "version", "invalid", "trailing"}:
        # Malformed/mismatched producer closes its record but remains alive: the
        # real parent must reject the data and TERM/reap it, not just await exit.
        sys.stdout.flush()
        os.close(1)
        signal.pause()
else:
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    print(CODE_MARKER + " PID " + str(os.getpid()), flush=True)
    if sys.argv[1:] == ["hold"]:
        signal.pause()
