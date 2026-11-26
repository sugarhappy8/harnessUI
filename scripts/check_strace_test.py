"""Tests for scripts/check_strace.py: strace text goes in, findings come out. No sockets are opened."""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
CHECKER = os.path.join(HERE, "check_strace.py")
BASE = "/work/real-rea"
CWD = "/work/repo"
TEST_BIN = "/work/repo/target/real_rea"


def check(*lines, allowed=TEST_BIN):
    """Run the checker on the given strace lines; return (exit code, stdout)."""
    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "strace.log")
        with open(log, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        p = subprocess.run(
            [sys.executable, "-I", CHECKER, log, BASE, CWD, allowed],
            capture_output=True, text=True, check=False,
        )
    return p.returncode, p.stdout


def start(pid=100):
    return f'{pid} execve("{TEST_BIN}", ["real_rea"], 0x0 /* 1 var */) = 0'


class OldRules(unittest.TestCase):
    def test_a_clean_log_passes(self):
        code, out = check(start(), '100 connect(3<UNIX:[1]>, {sa_family=AF_UNIX, sun_path="/run/x"}, 10) = 0')
        self.assertEqual(code, 0, out)

    def test_a_non_loopback_connect_fails(self):
        code, out = check(
            start(),
            '100 connect(5, {sa_family=AF_INET, sin_port=htons(9), sin_addr=inet_addr("192.0.2.1")}, 16) = -1 EINPROGRESS',
        )
        self.assertEqual(code, 1)
        self.assertIn("192.0.2.1", out)

    def test_a_name_lookup_through_the_loopback_resolver_fails(self):
        code, out = check(
            start(),
            '100 connect(5, {sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("127.0.0.53")}, 16) = 0',
        )
        self.assertEqual(code, 1)
        self.assertIn("name lookup", out)

    def test_loopback_passes(self):
        code, out = check(
            start(),
            '100 connect(5, {sa_family=AF_INET, sin_port=htons(8080), sin_addr=inet_addr("127.0.0.1")}, 16) = 0',
            '100 connect(6, {sa_family=AF_INET6, sin6_port=htons(8080), sin6_addr=inet_pton(AF_INET6, "::1")}, 28) = 0',
        )
        self.assertEqual(code, 0, out)

    def test_a_program_outside_the_pack_fails(self):
        code, out = check(start(), '100 execve("/usr/bin/curl", ["curl"], 0x0 /* 1 var */) = 0')
        self.assertEqual(code, 1)
        self.assertIn("started a program outside the pack", out)

    def test_ps_is_allowed(self):
        code, out = check(start(), '100 execve("/usr/bin/ps", ["ps"], 0x0 /* 1 var */) = 0')
        self.assertEqual(code, 0, out)

    def test_a_program_inside_the_pack_passes(self):
        code, out = check(start(), f'100 execve("{BASE}/kuna/kuna", ["kuna"], 0x0 /* 1 var */) = 0')
        self.assertEqual(code, 0, out)
        self.assertIn("<pack>/kuna/kuna", out)

    def test_a_write_outside_the_test_folder_fails(self):
        code, out = check(start(), '100 openat(AT_FDCWD, "/home/u/x.txt", O_WRONLY|O_CREAT, 0644) = 3</home/u/x.txt>')
        self.assertEqual(code, 1)
        self.assertIn("wrote outside the test's folder", out)

    def test_a_write_inside_the_test_folder_passes(self):
        code, out = check(start(), f'100 openat(AT_FDCWD, "{BASE}/a.txt", O_WRONLY|O_CREAT, 0644) = 3<{BASE}/a.txt>')
        self.assertEqual(code, 0, out)

    def test_java_performance_counters_and_devices_pass(self):
        code, out = check(
            start(),
            '100 openat(AT_FDCWD, "/tmp/hsperfdata_u/123", O_RDWR|O_CREAT, 0600) = 3</tmp/hsperfdata_u/123>',
            '100 openat(AT_FDCWD, "/dev/null", O_WRONLY, 0) = 4</dev/null>',
        )
        self.assertEqual(code, 0, out)

    def test_a_reading_open_is_not_a_write(self):
        code, out = check(start(), '100 openat(AT_FDCWD, "/etc/passwd", O_RDONLY) = 3</etc/passwd>')
        self.assertEqual(code, 0, out)

    def test_a_relative_mkdir_is_resolved_against_the_current_folder(self):
        code, out = check(start(), '100 mkdir("out", 0755) = 0')
        self.assertEqual(code, 1)
        self.assertIn(f"{CWD}/out", out)

    def test_a_rename_into_another_folder_fails(self):
        code, out = check(start(), f'100 rename("{BASE}/a", "/tmp/b") = 0')
        self.assertEqual(code, 1)

    def test_a_failed_call_is_not_a_finding(self):
        code, out = check(start(), '100 mkdir("/tmp/x", 0755) = -1 EACCES (Permission denied)')
        self.assertEqual(code, 0, out)

    def test_a_child_inherits_the_current_folder_through_clone(self):
        code, out = check(
            start(),
            f'100 chdir("{BASE}") = 0',
            "100 clone(child_stack=NULL, flags=SIGCHLD) = 101",
            '101 mkdir("sub", 0755) = 0',
        )
        self.assertEqual(code, 0, out)

    def test_an_unfinished_call_is_joined_with_its_resumption(self):
        code, out = check(
            start(),
            '100 connect(5, {sa_family=AF_INET, sin_port=htons(9), <unfinished ...>',
            '100 <... connect resumed>sin_addr=inet_addr("192.0.2.1")}, 16) = 0',
        )
        self.assertEqual(code, 1)


UDP4 = "5<UDP:[3.4.5.6:41234]>"
TCP = "5<TCP:[1]>"
V4 = lambda port, addr: f'{{sa_family=AF_INET, sin_port=htons({port}), sin_addr=inet_addr("{addr}")}}'
V6 = lambda port, addr: f'{{sa_family=AF_INET6, sin6_port=htons({port}), sin6_flowinfo=htonl(0), sin6_addr=inet_pton(AF_INET6, "{addr}"), sin6_scope_id=0}}'


class WithTheSocketKind(unittest.TestCase):
    def test_tcp_connect_to_a_non_loopback_address_fails(self):
        code, out = check(start(), f"100 connect({TCP}, {V4(9, '192.0.2.1')}, 16) = -1 EINPROGRESS")
        self.assertEqual(code, 1)
        self.assertIn("192.0.2.1", out)

    def test_udp_connect_to_port_0_passes(self):
        # glibc getaddrinfo sorting a host name's two addresses (RFC 6724): sends nothing.
        code, out = check(
            start(),
            f"100 connect({UDP4}, {V4(0, '10.1.0.32')}, 16) = 0",
            f"100 connect(6<UDPv6:[7]>, {V6(0, '2001:db8::1')}, 28) = 0",
        )
        self.assertEqual(code, 0, out)

    def test_udp_connect_to_another_port_fails(self):
        code, out = check(start(), f"100 connect({UDP4}, {V4(9, '192.0.2.1')}, 16) = 0")
        self.assertEqual(code, 1)
        self.assertIn("192.0.2.1", out)

    def test_udp_connect_to_port_0_over_ipv6_but_to_another_port_fails(self):
        code, _ = check(start(), f"100 connect(6<UDPv6:[7]>, {V6(443, '2001:db8::1')}, 28) = 0")
        self.assertEqual(code, 1)

    def test_a_tcp_connect_to_port_0_still_fails(self):
        code, _ = check(start(), f"100 connect({TCP}, {V4(0, '192.0.2.1')}, 16) = -1 EINVAL")
        self.assertEqual(code, 1)

    def test_a_name_lookup_fails(self):
        code, out = check(start(), f"100 connect({UDP4}, {V4(53, '127.0.0.53')}, 16) = 0")
        self.assertEqual(code, 1)
        self.assertIn("name lookup", out)

    def test_sendto_a_non_loopback_address_fails(self):
        code, out = check(start(), f'100 sendto({UDP4}, "x", 1, 0, {V4(9, "192.0.2.1")}, 16) = 1')
        self.assertEqual(code, 1)
        self.assertIn("192.0.2.1", out)

    def test_sendmsg_a_non_loopback_address_fails(self):
        code, _ = check(start(), f"100 sendmsg({UDP4}, {{msg_name={V4(9, '192.0.2.1')}, msg_namelen=16}}, 0) = 1")
        self.assertEqual(code, 1)

    def test_loopback_and_unix_sockets_pass(self):
        code, out = check(
            start(),
            f"100 connect({TCP}, {V4(8080, '127.0.0.1')}, 16) = 0",
            f"100 connect({UDP4}, {V4(9, '127.0.0.1')}, 16) = 0",
            f'100 connect(7<UNIX:[2]>, {{sa_family=AF_UNIX, sun_path="/run/x"}}, 10) = 0',
            f'100 sendto({UDP4}, "x", 1, 0, {V4(9, "127.0.0.1")}, 16) = 1',
        )
        self.assertEqual(code, 0, out)


class AddresslessSends(unittest.TestCase):
    """A send with no address goes to the socket's peer, which `-yy` prints (final review I1)."""

    # Recorded under the job's options, the address changed from 127.0.0.1 to 192.0.2.1.
    CONNECT_0 = '100 connect(3<UDP:[5010653]>, {sa_family=AF_INET, sin_port=htons(0), sin_addr=inet_addr("192.0.2.1")}, 16) = 0'

    def test_a_send_on_a_udp_socket_connected_to_port_0_fails(self):
        code, out = check(
            start(),
            self.CONNECT_0,
            '100 sendto(3<UDP:[127.0.0.1:38426->192.0.2.1:0]>, "exfil", 5, 0, NULL, 0) = 5',
        )
        self.assertEqual(code, 1, out)
        self.assertIn("sends to a non-loopback address", out)
        self.assertIn("192.0.2.1", out)

    def test_a_sendmsg_on_a_udp_socket_connected_to_port_0_fails(self):
        code, out = check(
            start(),
            self.CONNECT_0,
            "100 sendmsg(3<UDP:[127.0.0.1:44609->192.0.2.1:0]>, {msg_name=NULL, msg_namelen=0, "
            'msg_iov=[{iov_base="x", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 1',
        )
        self.assertEqual(code, 1, out)
        self.assertIn("192.0.2.1", out)

    def test_a_sendmmsg_and_an_ipv6_send_on_connected_sockets_fail(self):
        code, out = check(
            start(),
            "100 sendmmsg(3<UDP:[10.0.0.5:5000->192.0.2.1:0]>, [{msg_hdr={msg_name=NULL, msg_namelen=0, "
            'msg_iov=[{iov_base="x", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, msg_len=1}], 1, 0) = 1',
        )
        self.assertEqual(code, 1, out)
        code, out = check(
            start(),
            '100 sendto(6<UDPv6:[[2001:db8::5]:5000->[2001:db8::1]:0]>, "x", 1, 0, NULL, 0) = 1',
        )
        self.assertEqual(code, 1, out)
        self.assertIn("2001:db8::1", out)

    def test_a_send_on_a_tcp_socket_to_another_computer_fails(self):
        code, _ = check(start(), '100 sendto(5<TCP:[10.0.0.5:5000->192.0.2.1:443]>, "x", 1, 0, NULL, 0) = 1')
        self.assertEqual(code, 1)

    def test_addressless_sends_to_loopback_and_on_unix_sockets_pass(self):
        code, out = check(
            start(),
            '100 connect(3<UDP:[5061659]>, {sa_family=AF_INET, sin_port=htons(0), sin_addr=inet_addr("127.0.0.1")}, 16) = 0',
            '100 sendto(3<UDP:[127.0.0.1:57104->127.0.0.1:0]>, "exfil", 5, 0, NULL, 0) = 5',
            '100 sendto(6<UDPv6:[[::1]:5000->[::1]:0]>, "x", 1, 0, NULL, 0) = 1',
            '100 sendto(7<TCP:[127.0.0.1:5000->127.0.0.1:8080]>, "x", 1, 0, NULL, 0) = 1',
            '100 sendto(8<UNIX-STREAM:[11->12]>, "x", 1, MSG_NOSIGNAL, NULL, 0) = 1',
            '100 sendmsg(9<NETLINK:[ROUTE:20573]>, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12}, 0) = 20',
        )
        self.assertEqual(code, 0, out)

    def test_a_udp_send_whose_peer_strace_could_not_show_fails(self):
        # The socket's kind is known but not its peer: fail closed when the send went out.
        code, out = check(start(), '100 sendto(3<UDP:[5010653]>, "x", 1, 0, NULL, 0) = 1')
        self.assertEqual(code, 1, out)
        self.assertIn("peer", out)

    def test_a_refused_send_on_an_unconnected_udp_socket_is_not_a_finding(self):
        code, out = check(start(), '100 sendto(3<UDP:[5010653]>, "x", 1, 0, NULL, 0) = -1 EDESTADDRREQ (Destination address required)')
        self.assertEqual(code, 0, out)

    def test_a_udp_connect_to_port_0_without_the_socket_kind_fails(self):
        # Without -yy the socket's kind is unknown, so port 0 is not allowed (F1, deferred).
        code, out = check(
            start(),
            '100 connect(5, {sa_family=AF_INET, sin_port=htons(0), sin_addr=inet_addr("192.0.2.1")}, 16) = 0',
        )
        self.assertEqual(code, 1, out)
        self.assertIn("192.0.2.1", out)


class PayloadsAndEveryMessage(unittest.TestCase):
    """Addresses are read only from strace's own address structures, never from a payload, and
    every message of a send is judged on its own (security residuals, item 2). The lines were
    recorded with strace 6.8 under the job's options, in a network namespace whose loopback
    also held 192.0.2.2/24, so nothing left it."""

    CONNECT_0 = AddresslessSends.CONNECT_0

    def test_a_payload_cannot_pose_as_the_sockets_peer(self):
        # The peer annotation ends at the address shapes strace prints; the payload's
        # "->127.0.0.1:1]>" is not the peer.
        code, out = check(
            start(),
            self.CONNECT_0,
            '100 sendto(3<UDP:[192.0.2.2:47340->192.0.2.1:0]>, "->127.0.0.1:1]>", 15, 0, NULL, 0) = 15',
        )
        self.assertEqual(code, 1, out)
        self.assertIn("sends to a non-loopback address", out)

    def test_an_addressless_message_in_a_mixed_sendmmsg_goes_to_the_peer(self):
        code, out = check(
            start(),
            self.CONNECT_0,
            "100 sendmmsg(3<UDP:[192.0.2.2:53156->192.0.2.1:0]>, [{msg_hdr={msg_name={sa_family=AF_INET, "
            'sin_port=htons(9), sin_addr=inet_addr("127.0.0.1")}, msg_namelen=16, msg_iov=[{iov_base="a", '
            "iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, msg_len=1}, {msg_hdr={msg_name=NULL, "
            'msg_namelen=0, msg_iov=[{iov_base="b", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, '
            "msg_len=1}], 2, 0) = 2",
        )
        self.assertEqual(code, 1, out)
        self.assertIn("sends to a non-loopback address", out)

    def test_a_payload_cannot_pose_as_a_loopback_address(self):
        for payload in (r'"\"::ffff:127."', r'"\"::1\""', r'"inet_addr(\"127.0.0.1\")"', r'"\\\"::1\""'):
            code, out = check(
                start(),
                f"100 sendto(3<UDP:[5240869]>, {payload}, 12, 0, {V4(9, '192.0.2.1')}, 16) = 12",
            )
            self.assertEqual(code, 1, payload + out)
            self.assertIn("reaches a non-loopback address", out)

    def test_a_sendmmsg_mixing_loopback_and_non_loopback_addresses_fails(self):
        code, out = check(
            start(),
            "100 sendmmsg(3<UDP:[5247947]>, [{msg_hdr={msg_name={sa_family=AF_INET, sin_port=htons(9), "
            'sin_addr=inet_addr("127.0.0.1")}, msg_namelen=16, msg_iov=[{iov_base="a", iov_len=1}], '
            "msg_iovlen=1, msg_controllen=0, msg_flags=0}, msg_len=1}, {msg_hdr={msg_name={sa_family=AF_INET, "
            'sin_port=htons(9), sin_addr=inet_addr("192.0.2.1")}, msg_namelen=16, msg_iov=[{iov_base="b", '
            "iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, msg_len=1}], 2, 0) = 2",
        )
        self.assertEqual(code, 1, out)
        self.assertIn("reaches a non-loopback address", out)

    def test_a_sendmmsg_with_messages_strace_did_not_show_fails(self):
        # strace prints at most 32 messages of a vector, then "...".
        msg = (
            '{msg_hdr={msg_name={sa_family=AF_INET, sin_port=htons(9), sin_addr=inet_addr("127.0.0.1")}, '
            'msg_namelen=16, msg_iov=[{iov_base="m", iov_len=1}], msg_iovlen=1, msg_controllen=0, '
            "msg_flags=0}, msg_len=1}"
        )
        shown = ", ".join([msg] * 32)
        code, out = check(start(), f"100 sendmmsg(3<UDP:[5248537]>, [{shown}, ...], 40, 0) = 40")
        self.assertEqual(code, 1, out)
        self.assertIn("did not show", out)
        code, out = check(start(), f"100 sendmmsg(3<UDP:[5248537]>, [{shown}], 32, 0) = 32")
        self.assertEqual(code, 0, out)

    def test_an_addressless_message_on_a_socket_whose_peer_is_not_shown_fails_once_sent(self):
        line = (
            "100 sendmmsg(3<UDP:[5]>, [{msg_hdr={msg_name={sa_family=AF_INET, sin_port=htons(9), "
            'sin_addr=inet_addr("127.0.0.1")}, msg_namelen=16, msg_iov=[{iov_base="a", iov_len=1}], '
            "msg_iovlen=1, msg_controllen=0, msg_flags=0}, msg_len=1}, {msg_hdr={msg_name=NULL, msg_namelen=0, "
            'msg_iov=[{iov_base="b", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}{len}}], 2, 0) = {sent}'
        )
        code, out = check(start(), line.replace("{len}", ", msg_len=1").replace("{sent}", "2"))
        self.assertEqual(code, 1, out)
        self.assertIn("peer", out)
        # Only the first message went out: the second sent nothing.
        code, out = check(start(), line.replace("{len}", "").replace("{sent}", "1"))
        self.assertEqual(code, 0, out)

    def test_a_send_with_no_address_on_a_socket_whose_peer_is_not_shown_fails_whatever_its_kind(self):
        for sock in ("5<TCP:[1]>", "5<TCPv6:[[::1]:5000]>", "5<socket:[12]>", "5<RAW:[3]>", "5"):
            code, out = check(start(), f'100 sendto({sock}, "x", 1, 0, NULL, 0) = 1')
            self.assertEqual(code, 1, sock + out)
            self.assertIn("peer", out)

    def test_ipv6_addresses_as_strace_6_prints_them(self):
        v6 = lambda port, addr: (
            f'{{sa_family=AF_INET6, sin6_port=htons({port}), sin6_flowinfo=htonl(0), '
            f'inet_pton(AF_INET6, "{addr}", &sin6_addr), sin6_scope_id=0}}'
        )
        code, out = check(
            start(),
            f'100 sendto(6<UDPv6:[7]>, "x", 1, 0, {v6(9, "::1")}, 28) = 1',
            f'100 sendto(6<UDPv6:[7]>, "x", 1, 0, {v6(9, "::ffff:127.0.0.1")}, 28) = 1',
            f"100 connect(6<UDPv6:[7]>, {v6(0, '2001:db8::1')}, 28) = 0",
        )
        self.assertEqual(code, 0, out)
        code, out = check(start(), f'100 sendto(6<UDPv6:[7]>, "::1", 3, 0, {v6(9, "2001:db8::1")}, 28) = 3')
        self.assertEqual(code, 1, out)
        self.assertIn("2001:db8::1", out)

    def test_recorded_loopback_and_unix_sends_pass(self):
        code, out = check(
            start(),
            '100 sendto(3<UDP:[5247972]>, "x", 1, 0, {sa_family=AF_INET, sin_port=htons(9), sin_addr=inet_addr("127.0.0.1")}, 16) = 1',
            '100 connect(4<UDP:[5247973]>, {sa_family=AF_INET, sin_port=htons(0), sin_addr=inet_addr("127.0.0.1")}, 16) = 0',
            '100 sendto(4<UDP:[127.0.0.1:58661->127.0.0.1:0]>, "y", 1, 0, NULL, 0) = 1',
            '100 sendto(3<UNIX-STREAM:[5247978->5247979]>, "x", 1, 0, NULL, 0) = 1',
            '100 bind(3<UDP:[5251103]>, {sa_family=AF_INET, sin_port=htons(0), sin_addr=inet_addr("127.0.0.1")}, 16) = 0',
            '100 sendto(3<UDP:[127.0.0.1:44779]>, "w", 1, 0, {sa_family=AF_INET, sin_port=htons(9), sin_addr=inet_addr("127.0.0.1")}, 16) = 1',
            '100 sendto(3<UDP:[127.0.0.1:44779]>, "n", 1, 0, NULL, 0) = -1 EDESTADDRREQ (Destination address required)',
            '100 connect(5<TCP:[5251105]>, {sa_family=AF_INET, sin_port=htons(44803), sin_addr=inet_addr("127.0.0.1")}, 16) = 0',
            '100 sendto(5<TCP:[127.0.0.1:60990->127.0.0.1:44803]>, "tcp", 3, 0, NULL, 0) = 3',
            '100 sendto(6<TCP:[127.0.0.1:44803->127.0.0.1:60990]>, "back", 4, 0, NULL, 0) = 4',
            "100 sendto(7<NETLINK:[5251107]>, {nlmsg_len=0, nlmsg_type=0 /* NLMSG_??? */, nlmsg_flags=0, "
            "nlmsg_seq=0, nlmsg_pid=0}, 16, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 16",
            '100 bind(8<UNIX:[5251108]>, {sa_family=AF_UNIX, sun_path=@"probe-name"}, 13) = 0',
            '100 sendto(9<UNIX:[5251109]>, "u", 1, 0, {sa_family=AF_UNIX, sun_path=@"probe-name"}, 13) = 1',
            '100 sendto(10<UDP:[127.0.0.1:48181->127.0.0.1:9]>, "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"..., 100, 0, NULL, 0) = 100',
            "100 sendmmsg(3<UDP:[127.0.0.1:55649->127.0.0.1:0]>, [{msg_hdr={msg_name={sa_family=AF_INET, "
            'sin_port=htons(9), sin_addr=inet_addr("127.0.0.1")}, msg_namelen=16, msg_iov=[{iov_base="a", '
            "iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, msg_len=1}, {msg_hdr={msg_name=NULL, "
            'msg_namelen=0, msg_iov=[{iov_base="b", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}}], '
            "2, 0) = 1",
        )
        self.assertEqual(code, 0, out)


class FindingsNameTheirProgram(unittest.TestCase):
    def test_a_finding_names_its_program(self):
        code, out = check(
            start(),
            '100 execve("/usr/lib/jvm/bin/java", ["java"], 0x0 /* 1 var */) = 0',
            f"100 connect({TCP}, {V4(9, '192.0.2.1')}, 16) = -1 EINPROGRESS",
        )
        self.assertEqual(code, 1)
        self.assertIn("/usr/lib/jvm/bin/java (pid 100): reaches a non-loopback address", out)

    def test_a_child_is_named_after_the_program_that_started_it(self):
        code, out = check(
            start(),
            f'100 execve("{BASE}/jre/bin/java", ["java"], 0x0 /* 1 var */) = 0',
            "100 clone(child_stack=NULL, flags=SIGCHLD) = 101",
            f"101 connect({TCP}, {V4(9, '192.0.2.1')}, 16) = -1 EINPROGRESS",
        )
        self.assertEqual(code, 1)
        self.assertIn(f"{BASE}/jre/bin/java (pid 101): reaches", out)

    def test_a_process_is_named_after_its_last_execve(self):
        _, out = check(
            start(),
            f'100 execve("{BASE}/a", ["a"], 0x0 /* 1 var */) = 0',
            f'100 execve("{BASE}/b", ["b"], 0x0 /* 1 var */) = 0',
            f"100 connect({TCP}, {V4(9, '192.0.2.1')}, 16) = -1 EINPROGRESS",
        )
        self.assertIn(f"{BASE}/b (pid 100): reaches", out)

    def test_a_write_finding_names_its_program(self):
        _, out = check(start(), '100 mkdir("/tmp/x", 0755) = 0')
        self.assertIn(f"{TEST_BIN} (pid 100): wrote outside", out)


if __name__ == "__main__":
    unittest.main()
