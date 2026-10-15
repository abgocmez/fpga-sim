/* wsdyn.c - run-time bound winsock entry points; see wsdyn.h. */
#include <winsock2.h>
#include <ws2tcpip.h>

#include <stdio.h>

static HMODULE ws;

static void *sym(const char *name)
{
	FARPROC p;

	if (!ws) {
		ws = LoadLibraryA("ws2_32.dll");
		if (!ws) {
			fprintf(stderr, "wsdyn: cannot load ws2_32.dll\n");
			return NULL;
		}
	}
	p = GetProcAddress(ws, name);
	if (!p)
		fprintf(stderr, "wsdyn: ws2_32.dll has no %s\n", name);
	return (void *) p;
}

#define FN(ret, name, params, args, fail)                                \
	ret wsd_##name params                                            \
	{                                                                \
		static ret (WSAAPI *f) params;                           \
		if (!f)                                                  \
			f = (ret (WSAAPI *) params) sym(#name);          \
		return f ? f args : (fail);                              \
	}

FN(SOCKET, socket, (int af, int type, int proto), (af, type, proto), INVALID_SOCKET)
FN(int, connect, (SOCKET s, const struct sockaddr *a, int len), (s, a, len), SOCKET_ERROR)
FN(int, bind, (SOCKET s, const struct sockaddr *a, int len), (s, a, len), SOCKET_ERROR)
FN(int, listen, (SOCKET s, int backlog), (s, backlog), SOCKET_ERROR)
FN(SOCKET, accept, (SOCKET s, struct sockaddr *a, int *len), (s, a, len), INVALID_SOCKET)
FN(int, closesocket, (SOCKET s), (s), SOCKET_ERROR)
FN(int, recv, (SOCKET s, char *buf, int len, int flags), (s, buf, len, flags), SOCKET_ERROR)
FN(int, send, (SOCKET s, const char *buf, int len, int flags), (s, buf, len, flags), SOCKET_ERROR)
FN(int, select, (int n, fd_set *r, fd_set *w, fd_set *e, const struct timeval *t), (n, r, w, e, t), SOCKET_ERROR)
FN(int, WSAStartup, (WORD ver, LPWSADATA data), (ver, data), WSAVERNOTSUPPORTED)
FN(int, WSAGetLastError, (void), (), WSANOTINITIALISED)
FN(int, getaddrinfo, (const char *node, const char *svc, const struct addrinfo *hints, struct addrinfo **res),
   (node, svc, hints, res), WSANO_RECOVERY)

void wsd_freeaddrinfo(struct addrinfo *res)
{
	static void (WSAAPI *f)(struct addrinfo *);

	if (!f)
		f = (void (WSAAPI *)(struct addrinfo *)) sym("freeaddrinfo");
	if (f)
		f(res);
}
