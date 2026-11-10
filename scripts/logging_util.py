import datetime
import json
import sys


def log(message, severity="INFO", component="repository-tool"):
    assert severity in {"INFO", "WARNING", "ERROR", "DEBUG"}
    print(json.dumps({"timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      "severity": severity, "component": component, "message": message}), file=sys.stderr)
