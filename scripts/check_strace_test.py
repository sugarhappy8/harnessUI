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
