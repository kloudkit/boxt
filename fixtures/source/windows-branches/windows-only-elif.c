int read_setting(void)
{
    HKEY key;
#ifdef _WIN32
    return RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\App", 0, KEY_READ, &key);
#elif defined(_WIN64)
    return RegOpenKeyExW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\App", 0, KEY_READ, &key);
#endif
}
