#include <netinet/in.h>
#include <unistd.h>
#include <windows.h>

#define PORT 8443

void near_misses(uid_t uid, HWND window)
{
    unsigned short a = htons(8080);
    unsigned short b = htons(PORT);
    unsigned short c = htons(0);
    setuid(uid);
    RegisterHotKey(window, 1, MOD_ALT, 0x42);
    (void)a;
    (void)b;
    (void)c;
}
