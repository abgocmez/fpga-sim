/*
 * wsdyn.h - call winsock through LoadLibrary/GetProcAddress.
 *
 * xelab links the DPI code statically into xsimk.exe, and xsc cannot pass
 * -lws2_32 along; objects that carry static ws2_32 imports make xsimk.exe fail
 * to start (0xC0000139). Resolving the handful of functions at run time avoids
 * any import-table entry. Include this AFTER <winsock2.h>/<ws2tcpip.h>.
 */
#ifndef WSDYN_H
#define WSDYN_H

#include <winsock2.h>
#include <ws2tcpip.h>

SOCKET wsd_socket(int af, int type, int proto);
int    wsd_connect(SOCKET s, const struct sockaddr *a, int len);
int    wsd_bind(SOCKET s, const struct sockaddr *a, int len);
int    wsd_listen(SOCKET s, int backlog);
SOCKET wsd_accept(SOCKET s, struct sockaddr *a, int *len);
int    wsd_closesocket(SOCKET s);
int    wsd_recv(SOCKET s, char *buf, int len, int flags);
int    wsd_send(SOCKET s, const char *buf, int len, int flags);
int    wsd_select(int n, fd_set *r, fd_set *w, fd_set *e, const struct timeval *t);
int    wsd_WSAStartup(WORD ver, LPWSADATA data);
int    wsd_WSAGetLastError(void);
int    wsd_getaddrinfo(const char *node, const char *svc,
                       const struct addrinfo *hints, struct addrinfo **res);
void   wsd_freeaddrinfo(struct addrinfo *res);

#define socket          wsd_socket
#define connect         wsd_connect
#define bind            wsd_bind
#define listen          wsd_listen
#define accept          wsd_accept
#define closesocket     wsd_closesocket
#define recv            wsd_recv
#define send            wsd_send
#define select          wsd_select
#define WSAStartup      wsd_WSAStartup
#define WSAGetLastError wsd_WSAGetLastError
#define getaddrinfo     wsd_getaddrinfo
#define freeaddrinfo    wsd_freeaddrinfo

#endif
