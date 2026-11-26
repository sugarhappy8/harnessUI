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
# The socket a call is made on, as -yy prints it: 3<UDP:[127.0.0.1:38426->192.0.2.1:0]>,
# 6<UDPv6:[[::1]:5000->[2001:db8::1]:0]>, 3<UDP:[127.0.0.1:44779]> (bound, no peer), or
# 3<UDP:[5010653]> when strace shows neither. Only strace's own address shapes match, so the
# annotation can never run on into the call's arguments.
HOST = r"(?:\[[0-9a-fA-F:.]+\]|[0-9.]+)"
SOCKET = re.compile(r"^\w+\(\d+<(?:UDP|UDPv6|TCP|TCPv6):\[(?:" + HOST + r":\d+(?:->(" + HOST + r"):\d+)?|\d+)\]>")
# Sockets that stay on this computer: 8<UNIX-STREAM:[11->12]>, 7<UNIX:[2]>, 9<NETLINK:[ROUTE:20573]>.
LOCAL_SOCKET = re.compile(r"^\w+\(\d+<(?:UNIX|NETLINK)[-:]")
# A token of strace's text: an escaped character, or a quoted string (to the end of the line
# when it is cut off there).
TOKEN = re.compile(r'\\.|"(?:[^"\\]|\\.)*(?:"|$)')
# What strace prints just before a string that is an address it decoded from binary.
ADDRESS_BEFORE = ("inet_addr(", "inet_pton(AF_INET, ", "inet_pton(AF_INET6, ")
# A socket address as strace prints it: {sa_family=AF_INET, sin_port=htons(9),
# sin_addr=inet_addr("127.0.0.1")} or {sa_family=AF_INET6, sin6_port=htons(9), ...,
# inet_pton(AF_INET6, "::1", &sin6_addr), sin6_scope_id=0}.
SOCKADDR = re.compile(r"\{sa_family=(AF_[A-Z0-9]+)([^{}]*)\}")
HOST_IN = re.compile(r'inet_addr\("([^"]*)"\)|inet_pton\(AF_INET6?, "([^"]*)"')
PORT_IN = re.compile(r"sin6?_port=htons\((\d+)\)")
LOCAL_FAMILIES = ("AF_UNIX", "AF_LOCAL", "AF_NETLINK", "AF_UNSPEC")


def without_payload(call):
    """The call with every quoted string emptied but the addresses strace decodes itself, so
    no payload, path or name can read as an address or as a socket's peer."""
    def keep(m):
        t = m.group(0)
        if t.startswith('"') and not call.endswith(ADDRESS_BEFORE, 0, m.start()):
            return '""'
        return t
    return TOKEN.sub(keep, call)


def peer_of(text):
    """The socket a call is made on: ("local", None) for a Unix or netlink socket, ("ip", host)
    for an IP socket whose peer -yy printed, and ("unknown", None) for any other (no -yy, no
    peer shown, or a kind the checker does not know)."""
    m = SOCKET.match(text)
    if m and m.group(1):
        return "ip", m.group(1).strip("[]")
    if LOCAL_SOCKET.match(text):
        return "local", None
    return "unknown", None


def loopback_host(host):
    return host.startswith("127.") or host == "::1" or host.startswith("::ffff:127.")


def judge(name, part, peer, sent):
    """What is wrong with one message of a call (one connect or bind, one sendto or sendmsg,
    one message of a sendmmsg), or None. `part` holds no payload."""
    if "sa_family=" not in part:
        if name in ("connect", "bind"):
            return None
        # No address: the message goes to the socket's peer, which -yy prints (`write()` is not
        # traced; the connect was). A UDP socket connected to port 0 does send, so the peer
        # must be loopback; a peer that is not shown is unknown, and a sent message to it is a
        # finding, as a port-0 connect on a socket of unknown kind is.
        where, host = peer
        if where == "local" or (host is not None and loopback_host(host)):
            return None
        if host is not None:
            return "sends to a non-loopback address"
        return "a send whose peer strace did not show" if sent else None
    addresses = SOCKADDR.findall(part)
    if not addresses:
        return "an address the checker cannot read"
    for family, fields in addresses:
        if family in LOCAL_FAMILIES:
            continue
        port = PORT_IN.search(fields)
        port = int(port.group(1)) if port else None
        host = HOST_IN.search(fields)
        host = (host.group(1) or host.group(2)) if host else None
        if port == 53:
            # sin_port=htons(53): a name lookup, even through the loopback resolver.
            return "a name lookup"
        if name == "connect" and UDP_CONNECT.match(part) and port == 0:
            # glibc's getaddrinfo sorts a host name's addresses (RFC 6724) by connecting an
            # unbound UDP socket to each and sends nothing. A later send on such a socket is
            # judged by its peer.
            continue
        if family not in ("AF_INET", "AF_INET6") or host is None or not loopback_host(host):
            return f"{'listens on' if name == 'bind' else 'reaches'} a non-loopback address"
    return None


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
        text = without_payload(call)
        peer = peer_of(text)
        problems = []
        if name == "sendmmsg":
            # Each message carries its own address, or none; the first `code` were sent.
            parts = text.split("{msg_hdr=")[1:]
            if ok and code > len(parts):
                problems.append("a sendmmsg with messages strace did not show")
        else:
            parts = [text]
        for i, part in enumerate(parts):
            problems.append(judge(name, part, peer, ok and (name != "sendmmsg" or i < code)))
        for problem in dict.fromkeys(p for p in problems if p):
            if problem == "a name lookup":
                lookups.append(call.strip())
            finding(pid, f"{problem}: {call.strip()}")
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
