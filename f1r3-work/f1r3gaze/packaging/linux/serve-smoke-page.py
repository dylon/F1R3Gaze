"""Serve a deterministic page on an ephemeral loopback port for package smoke."""

import sys
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, _format, *_args):
        pass


directory = Path(sys.argv[1])
port_file = Path(sys.argv[2])
handler = partial(QuietHandler, directory=str(directory))
server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
port_file.write_text(str(server.server_port), encoding="ascii")
server.serve_forever()
