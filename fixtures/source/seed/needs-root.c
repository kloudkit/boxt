#include <sys/mount.h>
#include <unistd.h>

int become_root(void)
{
    if (setuid(0) != 0)
        return -1;
    return mount("none", "/mnt", "tmpfs", 0, "");
}

int become_root_fully(void)
{
    return setresuid(0, 0, 0);
}
