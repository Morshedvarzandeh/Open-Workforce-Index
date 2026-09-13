"""One service process owns the bounded worker queue; TLS is terminated by the host."""
import os

bind = '0.0.0.0:'+str(int(os.environ.get('PORT', '8080')))
workers = 1
threads = 6
timeout = 180
graceful_timeout = 160
accesslog = '-'
limit_request_line = 4096
limit_request_fields = 50
limit_request_field_size = 8190
umask = 0o077
