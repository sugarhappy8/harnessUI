/* The test program rea opens in the reverse-engineering tests (item5-spec.md WP9c, WP9e).
 * build.sh compiles it for Linux (hello-linux-x64, an ELF file with its symbols, so rea
 * finds `main`) and for Windows (hello-win-x64.exe, a PE file that imports three functions
 * from kernel32.dll, for inspect_pe_imports). It only prints one line. */

#ifdef _WIN32

typedef void *HANDLE;
typedef unsigned long DWORD;
typedef int BOOL;

__declspec(dllimport) HANDLE __stdcall GetStdHandle(DWORD std_handle);
__declspec(dllimport) BOOL __stdcall WriteFile(HANDLE file, const void *buffer, DWORD length,
                                               DWORD *written, void *overlapped);
__declspec(dllimport) void __stdcall ExitProcess(unsigned int code);

static const char greeting[] = "hello from harnessUI's test program\n";

int main(void) {
    DWORD written = 0;
    HANDLE out = GetStdHandle((DWORD)-11); /* STD_OUTPUT_HANDLE */
    WriteFile(out, greeting, (DWORD)(sizeof greeting - 1), &written, 0);
    return 0;
}

void mainCRTStartup(void) { ExitProcess((unsigned int)main()); }

#else

#include <stdio.h>

static const char *greeting(void) { return "hello from harnessUI's test program"; }

int main(void) {
    puts(greeting());
    return 0;
}

#endif
