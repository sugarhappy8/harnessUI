import os, re, sys
from collections import Counter

log, base, start_cwd, test_bin = sys.argv[1:5]
base = os.path.realpath(base)
test_bin = os.path.realpath(test_bin)
# rea lists its own processes with `ps`, found through the fixed PATH.
EXEC_OK = {"/usr/bin/ps", "/bin/ps"}
# Devices and the kernel's files, and Java's performance counters, which the JVM writes to
# /tmp/hsperfdata_<user> whatever its own temporary folder is.
WRITE_OK = re.compile(r"^(/dev/|/proc/|/tmp/hsperfdata_[^/]+(/[0-9]+)?$)")
QUOTED = r'"((?:[^"\\]|\\.)*)"'
DIRFD = r"(?:AT_FDCWD|-?\d+)(?:<([^>]*)>)?"
LINE = re.compile(r"^(\d+)\s+(.*)$")


def records():
    pending = {}
    with open(log, encoding="utf-8", errors="replace") as f:
        for raw in f:
            m = LINE.match(raw.rstrip("\n"))
            if not m:
                continue
            pid, rest = m.groups()
            if rest.endswith("<unfinished ...>"):
                pending[pid] = rest[: -len("<unfinished ...>")]
                continue
            r = re.match(r"^<\.\.\. \w+ resumed>(.*)$", rest)
            if r:
                rest = pending.pop(pid, "") + r.group(1)
            yield pid, rest


def unquote(s):
    return s.encode("latin-1", "replace").decode("unicode_escape", "replace")


def result(call):
    m = re.search(r"\)\s+=\s+(-?\d+)(?:<([^>]*)>)?", call)
    return (int(m.group(1)), m.group(2)) if m else (None, None)


# Pass 1: which process started which (clone, fork, vfork results).
parent = {}
for pid, call in records():
    if re.match(r"^(clone3?|v?fork)\(", call):
        code, _ = result(call)
        if code and code > 0:
            parent.setdefault(str(code), pid)

cwd = {}
first = None
# The last program each process ran (its latest successful execve); a child starts as its parent.
program = {}


def program_of(pid):
    while pid is not None and pid not in program:
        pid = parent.get(pid)
    return program[pid] if pid is not None else "unknown program"


def cwd_of(pid):
    if pid not in cwd:
        p = parent.get(pid)
        cwd[pid] = cwd_of(p) if p else (start_cwd if pid == first or first is None else None)
    return cwd[pid]


def resolve(path, dirfd_path, pid):
    path = unquote(path)
    if path.startswith("/"):
        return os.path.normpath(path)
    where = dirfd_path if dirfd_path is not None else cwd_of(pid)
    return os.path.normpath(os.path.join(where, path)) if where else None


def inside(path):
    return path == base or path.startswith(base + os.sep)


bad, notes, lookups, programs, seen_net = [], [], [], Counter(), 0


def finding(pid, text):
    bad.append(f"{program_of(pid)} (pid {pid}): {text}")


# With strace -yy a socket prints as 5<UDP:[...]>, 5<UDPv6:[...]>, 5<TCP:[...]> or 5<UNIX:[...]>.
UDP_CONNECT = re.compile(r"^connect\(\d+<UDP(?:v6)?:")
PORT_0 = re.compile(r"sin6?_port=htons\(0\)")
# The socket a call is made on, as -yy prints it: 3<UDP:[127.0.0.1:38426->192.0.2.1:0]>,
# 6<UDPv6:[[::1]:5000->[2001:db8::1]:0]>, or 3<UDP:[5010653]> when strace shows no peer.
SOCKET = re.compile(r"^\w+\(\d+<(UDP|UDPv6|TCP|TCPv6)(?::\[(.*)\])?>")


def peer_of(call):
    """For a call on an IP socket that -yy printed: (kind, the peer's host or None)."""
    m = SOCKET.match(call)
    if not m:
        return None, None
    kind, inner = m.group(1), m.group(2) or ""
    if "->" not in inner:
        return kind, None
    peer = inner.rsplit("->", 1)[1]
    host = peer[1:].split("]", 1)[0] if peer.startswith("[") else peer.rsplit(":", 1)[0]
    return kind, host


def loopback_host(host):
    return host.startswith("127.") or host == "::1" or host.startswith("::ffff:127.")


for pid, call in records():
    if first is None:
        first = pid
        cwd[pid] = start_cwd
    name = re.match(r"^(\w+)\(", call)
    if not name:
        continue
    name = name.group(1)
    code, fd_path = result(call)
    ok = code is not None and code >= 0
    if name in ("connect", "sendto", "sendmsg", "sendmmsg", "bind"):
        seen_net += 1
        family = re.search(r"sa_family=(AF_[A-Z0-9]+)", call)
        if family is None and name != "connect" and name != "bind":
            # A send with no address goes to the socket's peer (`write()` is not traced; the
            # connect was). A UDP socket connected to port 0 does send, so the peer is read
            # from the -yy annotation: not loopback is a finding, and so is a UDP send that
            # went out with no peer shown. Without -yy the kind is unknown, and every port-0
            # connect is already a finding.
            kind, host = peer_of(call)
            if host is not None and not loopback_host(host):
                finding(pid, f"sends to a non-loopback address: {call.strip()[:300]}")
            elif host is None and kind in ("UDP", "UDPv6") and ok:
                finding(pid, f"a UDP send whose peer strace did not show: {call.strip()[:300]}")
            continue
        if family is None or family.group(1) in ("AF_UNIX", "AF_LOCAL", "AF_NETLINK", "AF_UNSPEC"):
            continue
        loopback = re.search(r'inet_addr\("127\.|"::1"|"::ffff:127\.', call) is not None
        if "htons(53)" in call:
            lookups.append(call.strip())
            finding(pid, f"a name lookup: {call.strip()}")
        elif name == "connect" and UDP_CONNECT.match(call) and PORT_0.search(call):
            # glibc's getaddrinfo sorts a host name's addresses (RFC 6724) by connecting an
            # unbound UDP socket to each and sends nothing. A later send on such a socket is
            # checked above, by its peer.
            continue
        elif family.group(1) not in ("AF_INET", "AF_INET6") or not loopback:
            finding(pid, f"{'listens on' if name == 'bind' else 'reaches'} a non-loopback address: {call.strip()}")
    elif name in ("execve", "execveat"):
        m = re.search(QUOTED, call)
        path = resolve(m.group(1), None, pid) if m else None
        if not ok or path is None:
            continue
        if inside(path):
            programs["<pack>" + path[len(base):]] += 1
        else:
            programs[path] += 1
            if path != test_bin and path not in EXEC_OK:
                finding(pid, f"started a program outside the pack: {call.strip()[:300]}")
        program[pid] = path
    elif name in ("chdir", "fchdir"):
        if ok:
            if name == "chdir":
                m = re.search(QUOTED, call)
                new = resolve(m.group(1), None, pid) if m else None
            else:
                m = re.match(r"^fchdir\(" + DIRFD, call)
                new = m.group(1) if m else None
            cwd_of(pid)
            cwd[pid] = new
    else:
        target = None
        if name in ("open", "creat"):
            flags = call if name == "open" else "O_CREAT"
            if re.search(r"O_WRONLY|O_RDWR|O_CREAT|O_TRUNC", flags):
                m = re.match(r"^\w+\(" + QUOTED, call)
                target = fd_path or (resolve(m.group(1), None, pid) if m else None)
            else:
                continue
        elif name == "openat":
            m = re.match(r"^openat\(" + DIRFD + r", " + QUOTED + r", ([A-Z_|0-9]+)", call)
            if not m or not re.search(r"O_WRONLY|O_RDWR|O_CREAT|O_TRUNC", m.group(3)):
                continue
            target = fd_path or resolve(m.group(2), m.group(1), pid)
        elif name in ("mkdir",):
            m = re.match(r"^mkdir\(" + QUOTED, call)
            target = resolve(m.group(1), None, pid) if m else None
        elif name == "mkdirat":
            m = re.match(r"^mkdirat\(" + DIRFD + r", " + QUOTED, call)
            target = resolve(m.group(2), m.group(1), pid) if m else None
        elif name in ("rename", "link", "symlink"):
            m = re.match(r"^\w+\(" + QUOTED + r", " + QUOTED, call)
            target = resolve(m.group(2), None, pid) if m else None
        elif name in ("renameat", "renameat2", "linkat"):
            m = re.match(r"^\w+\(" + DIRFD + r", " + QUOTED + r", " + DIRFD + r", " + QUOTED, call)
            target = resolve(m.group(4), m.group(3), pid) if m else None
        elif name == "symlinkat":
            m = re.match(r"^symlinkat\(" + QUOTED + r", " + DIRFD + r", " + QUOTED, call)
            target = resolve(m.group(3), m.group(2), pid) if m else None
        else:
            continue
        if not ok:
            continue
        if target is None:
            notes.append(f"a write whose folder is not known: {call.strip()[:300]}")
        elif not inside(target) and not WRITE_OK.match(target):
            finding(pid, f"wrote outside the test's folder: {target} ({call.strip()[:200]})")

print(f"{seen_net} connect/sendto/sendmsg/sendmmsg/bind calls recorded")
print("Programs started (successful execve), with counts:")
for p, n in sorted(programs.items()):
    print(f"  {n:4d}  {p}")
for n in notes[:50]:
    print(f"::warning::{n}")
if bad:
    print("Not allowed (network, programs or writes outside the test's folder):")
    print("\n".join(bad[:80]))
    sys.exit(1)
print("Only loopback or Unix sockets, only the pack's programs (and ps), and writes only inside the test's folder.")
