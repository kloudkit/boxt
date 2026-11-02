#include <stdlib.h>

int read_setting(void)
{
#if defined(_WIN32) || defined(_WIN64)
    HKEY key;
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#else
    return getenv("APP_SETTING") != NULL;
#endif
}
