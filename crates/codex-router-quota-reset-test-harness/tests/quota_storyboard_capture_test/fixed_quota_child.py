"""Launch only the fixture-bound real TUI; retain exit/cleanup observations."""
import json
import os
import pathlib
import signal
import subprocess

configuration = json.loads(pathlib.Path(os.environ["QUOTA_STORYBOARD_FIXTURE_COMMAND"]).read_text())
output = pathlib.Path(configuration["output"])
(output / "quota-wrapper.pid").write_text(str(os.getpid()))
arguments = [configuration["binary"], "--router-root", configuration["root"],
    "--fixture-capability", configuration["capability"],
    "--provider-listener", configuration["provider"]]
environment = {"HOME": configuration["temporary_directory"], "TERM": "xterm-256color",
    "LANG": "C.UTF-8", "TMPDIR": configuration["temporary_directory"]}
child = subprocess.Popen(arguments, env=environment)
(output / "quota-child.pid").write_text(str(child.pid))

def terminate_owned_child(_signal_number: int, _frame: object) -> None:
    if child.poll() is None:
        child.terminate()

signal.signal(signal.SIGTERM, terminate_owned_child)
signal.signal(signal.SIGINT, terminate_owned_child)

def record_viewport(_signal_number: int = 0, _frame: object = None) -> None:
    viewport = os.get_terminal_size(1)
    (output / "quota-child.viewport").write_text(f"{viewport.columns} {viewport.lines}")

signal.signal(signal.SIGWINCH, record_viewport)
record_viewport()
exit_code = child.wait()
record_viewport()
(output / "quota-child.exit").write_text(str(exit_code))
print(f"QUOTA_STORYBOARD_CHILD_EXIT={exit_code}", flush=True)
raise SystemExit(exit_code)
