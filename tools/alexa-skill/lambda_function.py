"""Relais de la skill Alexa « maison connectée » vers Moli (AWS Lambda, Python 3.12+).

Il ne comprend rien : il transmet chaque directive d'Alexa à Moli (POST /api/alexa) avec le
jeton partagé, et rend la réponse de Moli. Tout le protocole est dans Moli
(crates/moli-api/src/alexa_home.rs).

Variables d'environnement de la fonction :
  MOLI_URL    l'adresse publique, par exemple https://alexa.example.org/api/alexa
  MOLI_TOKEN  le même jeton que le secret alexa-skill/token de Moli
"""

import json
import os
import urllib.error
import urllib.request
import uuid

URL = os.environ["MOLI_URL"]
TOKEN = os.environ["MOLI_TOKEN"]


def lambda_handler(event, context):
    request = urllib.request.Request(
        URL,
        data=json.dumps(event).encode(),
        headers={
            "Content-Type": "application/json",
            "Authorization": "Bearer " + TOKEN,
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=7) as answer:
            return json.load(answer)
    except (urllib.error.URLError, TimeoutError, ValueError) as error:
        print("Moli injoignable :", error)
        return unreachable(event)


def unreachable(event):
    """Moli ne répond pas : Alexa dit que l'appareil ne répond pas."""
    directive = event.get("directive", {})
    header = directive.get("header", {})
    answer = {
        "event": {
            "header": {
                "namespace": "Alexa",
                "name": "ErrorResponse",
                "payloadVersion": "3",
                "messageId": str(uuid.uuid4()),
            },
            "endpoint": {"endpointId": directive.get("endpoint", {}).get("endpointId", "")},
            "payload": {"type": "BRIDGE_UNREACHABLE", "message": "Moli ne répond pas"},
        }
    }
    if "correlationToken" in header:
        answer["event"]["header"]["correlationToken"] = header["correlationToken"]
    return answer
