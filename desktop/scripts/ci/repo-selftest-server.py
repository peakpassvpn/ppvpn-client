#!/usr/bin/env python3
"""The site and the release downloads, served from one local port, for the
self-test of the apt and dnf repositories (the `package-linux-repo` job of
desktop-package.yml). Not for anything published. Standard library only.

    repo-selftest-server.py --site DIR --assets DIR [--port 8080] [--bind 127.0.0.1]

    /download/<tag>/<file>   302 to /assets/<tag>/<file>, as a GitHub Release
                             download answers with a redirect to where the
                             file is stored
    /assets/<tag>/<file>     the file ASSETS/<tag>/<file>
    anything else            the file under SITE

/download is what build-linux-repo.sh gets as --download-base: a client that
does not follow the redirect cannot install the rpm. Requests are logged on
stderr, one line each with the status, which the job reads to see that the
redirect was taken.
"""

import argparse
import http.server
import os
import sys
import urllib.parse


class Handler(http.server.SimpleHTTPRequestHandler):
    site = ""
    assets = ""

    def redirected(self):
        """Answers a /download/ request; False when the request is not one."""
        path = urllib.parse.urlsplit(self.path).path
        if not path.startswith("/download/"):
            return False
        host = self.headers.get("Host") or "%s:%d" % self.server.server_address[:2]
        self.send_response(302)
        self.send_header("Location", f"http://{host}/assets/{path[len('/download/'):]}")
        self.send_header("Content-Length", "0")
        self.end_headers()
        return True

    def do_GET(self):
        if not self.redirected():
            super().do_GET()

    def do_HEAD(self):
        if not self.redirected():
            super().do_HEAD()

    def translate_path(self, path):
        # The parent resolves the request path, normalized, under self.directory.
        clean = urllib.parse.urlsplit(path).path
        if clean.startswith("/assets/"):
            self.directory = self.assets
            return super().translate_path(clean[len("/assets"):])
        self.directory = self.site
        return super().translate_path(clean)

    def list_directory(self, path):
        self.send_error(404, "no listing")
        return None


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--site", required=True)
    parser.add_argument("--assets", required=True)
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--bind", default="127.0.0.1")
    args = parser.parse_args(argv)
    for directory in (args.site, args.assets):
        if not os.path.isdir(directory):
            print(f"repo-selftest-server: {directory} is not a directory", file=sys.stderr)
            return 1
    Handler.site = os.path.abspath(args.site)
    Handler.assets = os.path.abspath(args.assets)
    server = http.server.ThreadingHTTPServer((args.bind, args.port), Handler)
    print(f"repo-selftest-server: http://{args.bind}:{args.port} (site {Handler.site}, assets {Handler.assets})",
          file=sys.stderr, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
