int read_setting(void)
{
#ifdef _WIN32
#ifdef _WIN64
    HKEY key;
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#else
    return read_win32_setting();
#endif
#endif
    return 0;
}
