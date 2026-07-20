#!/usr/bin/env python3
"""Exercise native adapter exception containment and same-process reconnect.

The test owns one loopback listener and one launcher PID. It never broad-kills
emulator processes.

Usage:
  native-adapter-failure-test.py flycast <content> [binary]
  native-adapter-failure-test.py mednafen --module pce <content> [binary]
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_FLYCAST = (
    Path.home()
    / "Library/Application Support/emucap/flycast-build/work/build/Flycast.app/Contents/MacOS/Flycast"
)
DEFAULT_MEDNAFEN = ROOT / "adapters/mednafen/work/mednafen/src/mednafen"


class Session:
    def __init__(self, socket_: socket.socket):
        self.socket = socket_
        self.socket.settimeout(30)
        self.file = socket_.makefile("rwb", buffering=0)
        self.next_id = 1

    def close(self) -> None:
        try:
            self.file.close()
        finally:
            self.socket.close()

    def call(self, method: str, params: dict | None = None) -> dict:
        request_id = self.next_id
        self.next_id += 1
        request = {
            "v": 1,
            "id": request_id,
            "method": method,
            "params": params or {},
        }
        self.file.write(json.dumps(request, separators=(",", ":")).encode() + b"\n")
        line = self.file.readline()
        if not line:
            raise RuntimeError(f"connection closed while waiting for {method}")
        response = json.loads(line)
        if response.get("id") != request_id:
            raise RuntimeError(f"response id mismatch for {method}: {response}")
        return response

    def wait_for_close(self) -> None:
        self.socket.settimeout(5)
        line = self.file.readline()
        if line:
            raise RuntimeError(f"old connection remained readable after containment: {line!r}")


def accept_session(
    listener: socket.socket, token: str, launch_id: str
) -> tuple[Session, dict]:
    socket_, _ = listener.accept()
    session = Session(socket_)
    hello = session.call("hello", {"session_token": token})
    if not hello.get("ok"):
        raise RuntimeError(f"hello failed: {hello}")
    result = hello["result"]
    if result.get("session_token") != token or result.get("launch_id") != launch_id:
        raise RuntimeError(f"identity mismatch: {result}")
    return session, result


def terminate_owned(pid: int) -> None:
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    for _ in range(30):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.1)
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def wait_for_file(path: Path, timeout: float = 5) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            return json.loads(path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            time.sleep(0.05)
    raise RuntimeError(f"failure artifact was not published: {path}")


def launch_owned(
    adapter: str,
    env: dict[str, str],
    content: Path,
    port: int,
    module: str | None,
    pidfile: Path,
) -> int:
    launcher = ROOT / f"adapters/{adapter}/launch.sh"
    command = [str(launcher), str(content), str(port)]
    if adapter == "mednafen":
        command.extend(["native-failure-test", module or ""])
    launched = subprocess.run(
        command,
        env=env,
        text=True,
        capture_output=True,
        timeout=45,
        check=False,
    )
    if launched.returncode != 0:
        raise RuntimeError(f"launch failed:\n{launched.stdout}\n{launched.stderr}")
    return int(pidfile.read_text().strip())


def validate_active_artifact(
    artifact: dict, adapter: str, content: Path, launch_id: str
) -> None:
    expected_adapter = f"{adapter}-native"
    expected = {
        "schema_version": 1,
        "kind": "adapter_internal_error",
        "adapter": expected_adapter,
        "launch_id": launch_id,
        "content": str(content),
        "operation": "service",
        "active": True,
        "execution_state": "unknown",
    }
    mismatches = {
        key: (artifact.get(key), value)
        for key, value in expected.items()
        if artifact.get(key) != value
    }
    if mismatches:
        raise RuntimeError(f"active failure artifact mismatch: {mismatches}")
    if "injected native adapter service exception" not in artifact.get("reason", ""):
        raise RuntimeError(f"failure reason lost exception detail: {artifact}")
    if not isinstance(artifact.get("frame"), int):
        raise RuntimeError(f"failure frame is missing: {artifact}")
    if not isinstance(artifact.get("observed_at_unix_ms"), int):
        raise RuntimeError(f"failure timestamp is missing: {artifact}")
    if artifact.get("truncated") is not False:
        raise RuntimeError(f"unexpected truncation in injected failure: {artifact}")
    if len(json.dumps(artifact).encode()) > 16 * 1024:
        raise RuntimeError("generic failure artifact exceeds its bounded schema")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("adapter", choices=("flycast", "mednafen"))
    parser.add_argument("--module", help="Mednafen force_module value")
    parser.add_argument("content")
    parser.add_argument("binary", nargs="?")
    args = parser.parse_args()

    content = Path(args.content).resolve()
    if args.binary:
        binary = Path(args.binary).resolve()
    elif args.adapter == "flycast":
        binary = DEFAULT_FLYCAST
    else:
        binary = DEFAULT_MEDNAFEN
    if not content.is_file() or not binary.is_file():
        raise SystemExit(f"missing content/binary: content={content} binary={binary}")

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(4)
    listener.settimeout(120)
    port = listener.getsockname()[1]
    token = "native-adapter-failure-test-token"
    launch_id = f"launch-{args.adapter}-native-failure-test"
    owned_pid: int | None = None

    with tempfile.TemporaryDirectory(prefix=f"emucap-{args.adapter}-native-failure-") as temp:
        home = Path(temp)
        generation = home / "sessions" / str(port) / "generations" / launch_id
        generation.mkdir(parents=True, mode=0o700)
        failure = generation / "adapter-failure.json"
        log = home / f"{args.adapter}-native-failure.log"
        env = os.environ.copy()
        env.update(
            {
                "EMUCAP_EMU_HOME": str(home),
                "EMUCAP_SESSION_TOKEN": token,
                "EMUCAP_LAUNCH_ID": launch_id,
                "EMUCAP_FAILURE_FILE": str(failure),
                "EMUCAP_LOG": str(log),
                "EMUCAP_ENABLE_TEST_ADAPTER_EXCEPTION": "1",
                "EMUCAP_HEADLESS": "1",
            }
        )
        if args.adapter == "flycast":
            env["FLYCAST_APP"] = str(binary)
        else:
            env.update(
                {
                    "MEDNAFEN_BIN": str(binary),
                    "MEDNAFEN_SOUND": "0",
                    "EMUCAP_POST_CONNECT_GRACE": "0",
                }
            )
        pidfile = home / args.adapter / str(port) / f"{args.adapter}.pid"
        owned_pid = launch_owned(
            args.adapter, env, content, port, args.module, pidfile
        )

        session: Session | None = None
        try:
            session, hello = accept_session(listener, token, launch_id)
            methods = set(hello.get("methods", []))
            if "test_adapter_exception" not in methods:
                raise RuntimeError(
                    f"test_adapter_exception not advertised under test gate: {methods}"
                )

            injected = session.call("test_adapter_exception")
            if injected.get("ok"):
                raise RuntimeError(f"injected exception unexpectedly succeeded: {injected}")
            if injected.get("error", {}).get("kind") != "internal_error":
                raise RuntimeError(f"injected exception was not structured: {injected}")
            if "injected native adapter service exception" not in injected["error"].get(
                "message", ""
            ):
                raise RuntimeError(f"structured error lost exception detail: {injected}")

            active = wait_for_file(failure)
            validate_active_artifact(active, args.adapter, content, launch_id)
            session.wait_for_close()
            session.close()
            session = None

            session, _ = accept_session(listener, token, launch_id)
            status = session.call("status")
            result = status.get("result", {})
            if not status.get("ok") or result.get("state") != "frozen":
                raise RuntimeError(f"reconnected status is not frozen: {status}")
            if result.get("adapter_failure_active") is not True:
                raise RuntimeError(f"status omitted the contained failure: {status}")
            if result.get("adapter_failure_operation") != "service":
                raise RuntimeError(f"status failure operation mismatch: {status}")

            recovered = wait_for_file(failure)
            deadline = time.monotonic() + 5
            while recovered.get("active") is not False and time.monotonic() < deadline:
                time.sleep(0.05)
                recovered = wait_for_file(failure)
            if recovered.get("active") is not False:
                raise RuntimeError(f"successful status did not mark recovery: {recovered}")
            if recovered.get("execution_state") != "frozen":
                raise RuntimeError(f"recovered state is not frozen: {recovered}")

            resumed = session.call("resume")
            if not resumed.get("ok"):
                raise RuntimeError(f"resume failed after containment: {resumed}")
            final_status = session.call("status")
            if final_status.get("result", {}).get("state") != "running":
                raise RuntimeError(f"adapter did not resume after containment: {final_status}")

            print(
                json.dumps(
                    {
                        "ok": True,
                        "adapter": args.adapter,
                        "pid": owned_pid,
                        "structured_error": True,
                        "old_connection_closed": True,
                        "same_process_reconnected": True,
                        "contained_state": "frozen",
                        "artifact_recovered": True,
                        "resumed": True,
                        "failure_bytes": failure.stat().st_size,
                    },
                    separators=(",", ":"),
                )
            )
        finally:
            if session is not None:
                session.close()
            if owned_pid is not None:
                terminate_owned(owned_pid)
            listener.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
