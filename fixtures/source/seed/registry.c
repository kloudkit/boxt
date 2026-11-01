#ifdef _WIN32
#include <windows.h>

int read_setting(DWORD *out)
{
    HKEY key;
    DWORD size = sizeof *out;
    if (RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key) != ERROR_SUCCESS)
        return -1;
    LONG rc = RegQueryValueExA(key, "Port", NULL, NULL, (LPBYTE)out, &size);
    RegCloseKey(key);
    return rc == ERROR_SUCCESS ? 0 : -1;
}
#endif
