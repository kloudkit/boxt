int read_setting(void)
{
#ifndef NOREG
#ifdef _WIN32
    HKEY key;
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#endif
#else
    return 0;
#endif
    return -1;
}
