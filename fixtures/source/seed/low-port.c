#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>

int listen_http(void)
{
    struct sockaddr_in addr;
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    memset(&addr, 0, sizeof addr);
    addr.sin_family = AF_INET;
    addr.sin_port = htons(80);
    if (bind(fd, (struct sockaddr *)&addr, sizeof addr) != 0)
        return -1;
    return listen(fd, 16);
}
