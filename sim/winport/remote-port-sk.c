/*
 * Windows (winsock) replacement for libsystemctlm-soc libremote-port/remote-port-sk.c.
 * Same sk_open() contract; only TCP is supported ("tcp:host:port" connects,
 * "tcpd:host:port" listens and accepts one peer). AF_UNIX is not used on
 * Windows; start QEMU with a TCP chardev instead.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

#include <winsock2.h>
#include <ws2tcpip.h>
#include "wsdyn.h"

#include "remote-port-sk.h"

#define UNIX_PREFIX "unix:"
#define TCP_PREFIX "tcp:"
#define TCPD_PREFIX "tcpd:"

static int sk_wsa_init(void)
{
	static int done;
	WSADATA wsa;

	if (done)
		return 0;
	if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) {
		fprintf(stderr, "WSAStartup failed\n");
		return -1;
	}
	done = 1;
	return 0;
}

static int sk_tcp_client(const char *descr, bool daemon)
{
	struct addrinfo hints;
	struct addrinfo *result, *rp;
	SOCKET sfd = INVALID_SOCKET;
	char *host, *port = NULL, *pos;
	size_t prefix_len = daemon ? strlen(TCPD_PREFIX) : strlen(TCP_PREFIX);
	int s, ret = -1;

	if (sk_wsa_init())
		return -1;

	pos = (char *) descr + prefix_len;
	while (*pos == '/')
		pos++;

	host = strdup(pos);
	pos = strchr(host, ':');
	if (pos) {
		*pos = 0;
		port = pos + 1;
	}

	memset(&hints, 0, sizeof hints);
	hints.ai_family = AF_UNSPEC;
	hints.ai_socktype = SOCK_STREAM;
	hints.ai_flags = daemon ? AI_PASSIVE : 0;

	s = getaddrinfo(host, port, &hints, &result);
	if (s != 0) {
		fprintf(stderr, "getaddrinfo(%s): error %d\n", host, s);
		free(host);
		return -1;
	}

	for (rp = result; rp != NULL; rp = rp->ai_next) {
		sfd = socket(rp->ai_family, rp->ai_socktype, rp->ai_protocol);
		if (sfd == INVALID_SOCKET)
			continue;

		if (daemon) {
			SOCKET lfd = sfd;

			if (bind(lfd, rp->ai_addr, (int) rp->ai_addrlen) == SOCKET_ERROR ||
			    listen(lfd, 1) == SOCKET_ERROR) {
				fprintf(stderr, "bind/listen failed: %d\n", WSAGetLastError());
				closesocket(lfd);
				break;
			}
			printf("Waiting on connections to %s:%s\n", host, port ? port : "");
			sfd = accept(lfd, NULL, NULL);
			closesocket(lfd);
			if (sfd == INVALID_SOCKET) {
				fprintf(stderr, "accept failed: %d\n", WSAGetLastError());
				break;
			}
			ret = (int) sfd;
			break;
		}

		if (connect(sfd, rp->ai_addr, (int) rp->ai_addrlen) != SOCKET_ERROR) {
			ret = (int) sfd;
			break;
		}
		closesocket(sfd);
		sfd = INVALID_SOCKET;
	}

	freeaddrinfo(result);
	free(host);
	return ret;
}

int sk_open(const char *descr)
{
	if (descr == NULL)
		return -1;

	if (memcmp(UNIX_PREFIX, descr, strlen(UNIX_PREFIX)) == 0) {
		fprintf(stderr, "remote-port: unix: sockets are not supported on "
			"Windows; use tcp:host:port (%s)\n", descr);
		return -1;
	}
	if (memcmp(TCPD_PREFIX, descr, strlen(TCPD_PREFIX)) == 0)
		return sk_tcp_client(descr, true);
	if (memcmp(TCP_PREFIX, descr, strlen(TCP_PREFIX)) == 0)
		return sk_tcp_client(descr, false);
	return -1;
}
