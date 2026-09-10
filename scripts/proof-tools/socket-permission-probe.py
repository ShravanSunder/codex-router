"""Verify denied connections to two live, test-owned endpoints from an agent sandbox."""

import socket
import sys


def require_connection_denied(
    family: socket.AddressFamily, address: str | tuple[str, int], marker: str
) -> None:
    try:
        with socket.socket(family, socket.SOCK_STREAM) as connection:
            connection.connect(address)
    except PermissionError:
        print(marker)
        return
    raise RuntimeError(f"Sandbox unexpectedly allowed {marker}")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("Usage: socket-permission-probe.py UNIX_SOCKET TCP_PORT")
    _, socket_path, port_text = sys.argv
    port = int(port_text)
    if not 1 <= port <= 65535:
        raise ValueError("TCP port must be 1..65535")
    require_connection_denied(socket.AF_UNIX, socket_path, "SOCKET_BOUNDARY_OK")
    require_connection_denied(socket.AF_INET, ("127.0.0.1", port), "TCP_BOUNDARY_OK")


if __name__ == "__main__":
    main()
