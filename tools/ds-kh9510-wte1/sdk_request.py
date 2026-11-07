#!/usr/bin/env python3
"""One authenticated ISAPI request over the vendor HCNetSDK (Linux amd64)."""
import argparse
import ctypes as C
import getpass
from pathlib import Path
import sys

DWORD, BYTE, PTR = C.c_uint32, C.c_ubyte, C.c_void_p


class Input(C.Structure):
    _fields_ = [('size', DWORD), ('url', PTR), ('urlsize', DWORD),
                ('body', PTR), ('bodysize', DWORD), ('timeout', DWORD),
                ('encrypt', BYTE), ('multi', BYTE), ('mime', BYTE), ('reserved', BYTE * 29)]


class Output(C.Structure):
    _fields_ = [('size', DWORD), ('out', PTR), ('outsize', DWORD),
                ('returned', DWORD), ('status', PTR), ('statussize', DWORD),
                ('reserved', BYTE * 32)]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--host', required=True)
    p.add_argument('--port', type=int, default=8000)
    p.add_argument('--user', default='admin')
    p.add_argument('--method', choices=['GET', 'PUT', 'POST', 'DELETE'], required=True)
    p.add_argument('--path', required=True)
    p.add_argument('--body-file', type=Path)
    a = p.parse_args()
    if not a.path.startswith('/ISAPI/') or any(c.isspace() for c in a.path):
        p.error('path must begin /ISAPI/ and contain no whitespace')
    if not 1 <= a.port <= 65535:
        p.error('invalid port')
    body_bytes = a.body_file.read_bytes() if a.body_file else b''
    sdk = C.CDLL('/sdk/libhcnetsdk.so')
    signatures = {
        'NET_DVR_Init': ([], C.c_int32),
        'NET_DVR_Cleanup': ([], C.c_int32),
        'NET_DVR_SetConnectTime': ([DWORD, DWORD], C.c_int32),
        'NET_DVR_GetLastError': ([], DWORD),
        'NET_DVR_Login_V30': ([C.c_char_p, C.c_ushort, C.c_char_p, C.c_char_p, PTR], C.c_int32),
        'NET_DVR_Logout': ([C.c_int32], C.c_int32),
        'NET_DVR_STDXMLConfig': ([C.c_int32, C.POINTER(Input), C.POINTER(Output)], C.c_int32),
    }
    for name, (args, result) in signatures.items():
        fn = getattr(sdk, name)
        fn.argtypes, fn.restype = args, result
    if not sdk.NET_DVR_Init():
        raise RuntimeError(f'SDK init failed: {sdk.NET_DVR_GetLastError()}')
    session = -1
    try:
        sdk.NET_DVR_SetConnectTime(4000, 1)
        info = C.create_string_buffer(4096)
        password = getpass.getpass('DS-KH9510-WTE1 admin password: ')
        session = sdk.NET_DVR_Login_V30(a.host.encode(), a.port, a.user.encode(),
                                       password.encode(), info)
        del password
        if session < 0:
            raise RuntimeError(f'SDK login failed: {sdk.NET_DVR_GetLastError()}')
        url = C.create_string_buffer(f'{a.method} {a.path}'.encode())
        body = C.create_string_buffer(body_bytes)
        response, status = C.create_string_buffer(131072), C.create_string_buffer(4096)
        request, reply = Input(), Output()
        request.size, request.url, request.urlsize = C.sizeof(request), C.addressof(url), len(url) - 1
        request.body, request.bodysize, request.timeout = C.addressof(body), len(body_bytes), 15000
        reply.size = C.sizeof(reply)
        reply.out, reply.outsize = C.addressof(response), len(response)
        reply.status, reply.statussize = C.addressof(status), len(status)
        ok = sdk.NET_DVR_STDXMLConfig(session, C.byref(request), C.byref(reply))
        error = sdk.NET_DVR_GetLastError()
        if reply.returned > len(response):
            raise RuntimeError('SDK returned an oversized response')
        sys.stdout.buffer.write(response.raw[:reply.returned].rstrip(b'\0') + b'\n')
        if status.value:
            print(status.value.decode(errors='replace'), file=sys.stderr)
        if not ok:
            raise RuntimeError(f'SDK request failed: {error}')
        # A successful SDK call can still carry a device-level error. Inspect status.
    finally:
        if session >= 0:
            sdk.NET_DVR_Logout(session)
        sdk.NET_DVR_Cleanup()


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError) as error:
        sys.exit(str(error))
