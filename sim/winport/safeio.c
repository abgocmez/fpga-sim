/*
 * Windows replacement for libsystemctlm-soc libremote-port/safeio.c.
 * The descriptors handed to rp_safe_read/rp_safe_write are winsock SOCKETs
 * (see winport/remote-port-sk.c), so use recv()/send(). rp_safe_copyfd() is
 * not used by the remote-port TLM adaptor and is not provided.
 */
#include <errno.h>
#include <stddef.h>
#include <sys/types.h>

#include <winsock2.h>
#include "wsdyn.h"

#include "safeio.h"

ssize_t rp_safe_read(int fd, void *rbuf, size_t count)
{
	char *buf = (char *) rbuf;
	size_t rlen = 0;

	while (rlen < count) {
		int r = recv((SOCKET) fd, buf + rlen, (int) (count - rlen), 0);

		if (r < 0)
			return -1;
		if (r == 0)		/* peer closed */
			return (ssize_t) rlen;
		rlen += (size_t) r;
	}
	return (ssize_t) rlen;
}

ssize_t rp_safe_write(int fd, const void *wbuf, size_t count)
{
	const char *buf = (const char *) wbuf;
	size_t wlen = 0;

	while (wlen < count) {
		int w = send((SOCKET) fd, buf + wlen, (int) (count - wlen), 0);

		if (w < 0)
			return -1;
		wlen += (size_t) w;
	}
	return (ssize_t) wlen;
}
