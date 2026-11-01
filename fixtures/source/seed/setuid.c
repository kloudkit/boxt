#include <grp.h>
#include <sys/types.h>
#include <unistd.h>

int drop_to(const char *user, uid_t uid, gid_t gid)
{
    if (initgroups(user, gid) != 0)
        return -1;
    if (setresgid(gid, gid, gid) != 0)
        return -1;
    return setresuid(uid, uid, uid);
}
