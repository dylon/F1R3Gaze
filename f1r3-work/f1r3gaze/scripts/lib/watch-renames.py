#!/usr/bin/env python3
"""watch-renames.py — log every file renamed into a folder (Linux, inotify).

    python3 scripts/lib/watch-renames.py DIR LOG

Until it is killed, it appends one line per file moved into DIR (inotify's
IN_MOVED_TO, which an atomic write's rename produces) to LOG:

    <seconds since the epoch, to the microsecond> TAB <file name>

DIR must exist. `scripts/resize-bench.sh --watch-saves` runs it on the
profile's `state/` folder, to count the writes of `window.json` during a
resize sweep (storage ledger S13, part 2), from outside the process: the
frame log's `window_save` lines are the window's readings, and a reading
writes only when what `window.json` holds changed.

It uses libc through ctypes, so it needs no package (inotify-tools is not
installed everywhere).
"""

import ctypes
import ctypes.util
import os
import struct
import sys
import time

IN_MOVED_TO = 0x00000080
IN_CLOEXEC = 0o2000000
# struct inotify_event { int wd; uint32_t mask, cookie, len; char name[]; }
HEADER = struct.Struct("iIII")


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 64
    folder, log = sys.argv[1], sys.argv[2]
    libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
    fd = libc.inotify_init1(IN_CLOEXEC)
    if fd < 0:
        print(f"inotify_init1: {os.strerror(ctypes.get_errno())}", file=sys.stderr)
        return 1
    if libc.inotify_add_watch(fd, os.fsencode(folder), IN_MOVED_TO) < 0:
        print(f"inotify_add_watch {folder}: {os.strerror(ctypes.get_errno())}", file=sys.stderr)
        return 1
    with open(log, "a", buffering=1) as out:
        while True:
            data = os.read(fd, 65536)
            now = time.time()
            offset = 0
            while offset + HEADER.size <= len(data):
                _, mask, _, length = HEADER.unpack_from(data, offset)
                name = data[offset + HEADER.size : offset + HEADER.size + length].rstrip(b"\0").decode(errors="replace")
                offset += HEADER.size + length
                if mask & IN_MOVED_TO:
                    out.write(f"{now:.6f}\t{name}\n")


if __name__ == "__main__":
    sys.exit(main())
