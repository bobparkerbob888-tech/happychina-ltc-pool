#!/usr/bin/env python3
import json, http.server, urllib.parse, psycopg2

DB = "dbname=happychain user=pool password=pool host=127.0.0.1"

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        parts = parsed.path.strip('/').split('/')
        # /best-shares/<address>
        if len(parts) == 2 and parts[0] == 'best-shares':
            addr = parts[1]
            try:
                conn = psycopg2.connect(DB)
                cur = conn.cursor()
                cur.execute(
                    'SELECT worker, MAX(share_difficulty) as best_share '
                    'FROM shares WHERE miner = %s GROUP BY worker',
                    (addr,)
                )
                rows = cur.fetchall()
                result = {r[0]: r[1] for r in rows}
                cur.close()
                conn.close()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps(result).encode())
            except Exception as e:
                self.send_response(500)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'error': str(e)}).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        pass  # silence logs

if __name__ == '__main__':
    server = http.server.HTTPServer(('127.0.0.1', 8091), Handler)
    print('Best-shares API on :8091')
    server.serve_forever()
