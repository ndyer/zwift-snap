#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

/* Use Wine's window API so detection works on both X11 and native Wayland. */
struct window_search {
    DWORD pid;
    BOOL found;
};

static BOOL CALLBACK find_window(HWND window, LPARAM data)
{
    struct window_search *search = (struct window_search *)data;
    DWORD pid;
    RECT rect;

    GetWindowThreadProcessId(window, &pid);
    if (pid == search->pid && IsWindowVisible(window) && !IsIconic(window) &&
        GetWindowRect(window, &rect) && rect.right > rect.left && rect.bottom > rect.top) {
        search->found = TRUE;
        return FALSE;
    }
    return TRUE;
}

int main(int argc, char **argv)
{
    char *end;
    unsigned long pid;
    HANDLE process;
    ULONGLONG deadline;
    struct window_search search = {0, FALSE};

    if (argc != 2 || !(pid = strtoul(argv[1], &end, 10)) || *end) {
        fprintf(stderr, "Usage: wait-for-window.exe PID\n");
        return 1;
    }
    search.pid = pid;
    process = OpenProcess(SYNCHRONIZE, FALSE, search.pid);
    if (!process) {
        fprintf(stderr, "Cannot open process %lu: %lu\n", search.pid, GetLastError());
        return 1;
    }
    deadline = GetTickCount64() + 120000;
    for (;;) {
        if (!EnumWindows(find_window, (LPARAM)&search) && !search.found) {
            fprintf(stderr, "Cannot enumerate windows: %lu\n", GetLastError());
            CloseHandle(process);
            return 1;
        }
        if (search.found) {
            CloseHandle(process);
            return 0;
        }
        DWORD status = WaitForSingleObject(process, 100);
        if (status != WAIT_TIMEOUT) {
            fprintf(stderr, "Process %lu exited or cannot be waited on (status %lu)\n", search.pid, status);
            CloseHandle(process);
            return 1;
        }
        if (GetTickCount64() >= deadline) {
            fprintf(stderr, "Timed out waiting for process %lu to show a window\n", search.pid);
            CloseHandle(process);
            return 2;
        }
    }
}
