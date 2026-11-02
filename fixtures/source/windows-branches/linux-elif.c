#include <stdlib.h>

int read_setting(void)
{
#ifdef _WIN32
    HKEY key;
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#elif defined(__linux__)
    return getenv("APP_SETTING") != NULL;
#endif
}
