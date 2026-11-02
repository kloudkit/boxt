#include <stdlib.h>

int read_setting(void)
{
#ifdef USE_REG
#ifdef _WIN32
    HKEY key;
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#else
    return getenv("APP_SETTING") != NULL;
#endif
#endif
    return 0;
}
