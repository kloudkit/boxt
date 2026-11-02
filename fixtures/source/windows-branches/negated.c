#include <stdlib.h>

int read_setting(void)
{
#ifndef _WIN32
    return getenv("APP_SETTING") != NULL;
#else
    HKEY key;
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#endif
}
