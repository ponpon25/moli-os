#!/usr/bin/env python3
"""What the Tuya cloud knows about the home's devices, for `moli-os tuya import`,
without Home Assistant: one QR login, scanned with the Tuya Smart / Smart Life app.

Same login as Home Assistant's Tuya integration (its public client id): the app
shows a « authorise » screen, nothing else is linked to the account, and the
session lives only in this process (never written, never refreshed).

    docker run --rm -i -v /tmp/tuya-qr:/qr python:3.12-slim sh -c \\
        'pip -q install tuya-device-sharing-sdk cryptography qrcode pillow >&2 && python3 - <USER_CODE> --summary' < qr-export.py
    … same without --summary | docker exec -i moli-os /moli-os tuya import      # keys never shown

Without --summary the summary still goes to stderr: one scan is enough.

<USER_CODE>: app → Me → settings → Account and Security → User Code.
The QR is written to /qr/login.png (and drawn on stderr); scan it within 2 min.
The JSON holds the devices' local keys: only ever pipe it.
"""
import json
import sys
import time

import qrcode
from tuya_sharing import LoginControl, Manager, SharingTokenListener

CLIENT_ID = "HA_3y9q4ak7g4ephrvke"
SCHEMA = "haauthorize"
WAIT_S = 120
TYPES = {"Boolean": "Boolean", "Integer": "Integer", "Enum": "Enum", "String": "String",
         "Json": "Json", "Raw": "Raw", "Bitmap": "Bitmap"}


class NoRefresh(SharingTokenListener):
    def update_token(self, token_info):
        pass


def numeric(value):
    """Tuya sends numbers, or numbers as text."""
    try:
        return None if value in (None, "") else float(value)
    except (TypeError, ValueError):
        return None


def dps_of(device):
    writable = set((getattr(device, "function", None) or {}).keys())
    dps = []
    for dp_id, item in (getattr(device, "local_strategy", None) or {}).items():
        code = item.get("status_code")
        conf = item.get("config_item") or {}
        if not code:
            continue
        try:
            desc = json.loads(conf.get("valueDesc") or "{}")
        except ValueError:
            desc = {}
        dp = {"id": int(dp_id), "code": code, "type": TYPES.get(conf.get("valueType"), "Raw"),
              "writable": code in writable}
        if desc.get("unit") not in (None, ""):
            dp["unit"] = str(desc["unit"])
        for key in ("scale", "min", "max", "step"):
            number = numeric(desc.get(key))
            if number is not None:
                dp[key] = int(number) if key == "scale" else number
        if desc.get("range"):
            dp["range"] = [str(v) for v in desc["range"]]
        dps.append(dp)
    return sorted(dps, key=lambda d: d["id"])


def login(user_code):
    control = LoginControl()
    response = control.qr_code(CLIENT_ID, SCHEMA, user_code)
    if not response.get("success"):
        sys.exit(f"Tuya refused the user code: {response.get('msg') or response.get('code')}")
    token = response["result"]["qrcode"]
    qr = qrcode.QRCode(border=2)
    qr.add_data(f"tuyaSmart--qrLogin?token={token}")
    qr.make_image().save("/qr/login.png")
    qr.print_ascii(out=sys.stderr)
    print("Scan this QR with the Tuya app (+ → scan), then confirm.", file=sys.stderr)
    end = time.time() + WAIT_S
    while time.time() < end:
        time.sleep(3)
        ok, info = control.login_result(token, CLIENT_ID, user_code)
        if ok:
            return info
    sys.exit("no scan within 2 min: run again")


def main(user_code, summary):
    info = login(user_code)
    token = {k: info[k] for k in ("t", "uid", "expire_time", "access_token", "refresh_token")}
    manager = Manager(CLIENT_ID, user_code, info["terminal_id"], info["endpoint"], token, NoRefresh())
    manager.update_device_cache()
    devices = []
    for d in manager.device_map.values():
        out = {"id": d.id, "name": (d.name or d.id).strip(), "category": d.category or "",
               "product": (d.product_name or "").strip() or None, "dps": dps_of(d),
               "local_key": getattr(d, "local_key", None)}
        if getattr(d, "sub", False) or getattr(d, "node_id", None):
            out["gateway"] = "tuya"
        devices.append(out)
    # The summary (no key) always goes to stderr, so one scan both shows and imports.
    for d in devices:
        print(f'{d["id"][-6:]:8} {d["category"]:6} {len(d["dps"]):2} DPs '
              f'key:{"yes" if d["local_key"] else "no "} {"sub " if "gateway" in d else "    "}{d["name"]}',
              file=sys.stderr)
        for dp in d["dps"]:
            print(f'           {dp["id"]:>3} {dp["code"]:24} {dp["type"]:8} '
                  f'{"rw" if dp["writable"] else "r "} {dp.get("unit", "")} {dp.get("range", "")}', file=sys.stderr)
    print(f"{len(devices)} devices", file=sys.stderr)
    if not summary:
        json.dump({"devices": devices}, sys.stdout, ensure_ascii=False)


args = [a for a in sys.argv[1:] if not a.startswith("--")]
if not args:
    sys.exit("usage: qr-export.py <USER_CODE> [--summary]")
main(args[0], "--summary" in sys.argv)
