# UniFi Protect

- Origine : rien de noté (API d'intégration de la console : `/proxy/protect/integration/v1`,
  clé dans l'en-tête `X-API-KEY`).
- `fixtures/cameras.json` : la liste de caméras d'une vraie Cloud Key Gen2 Plus (Protect 7.3.70),
  adresses MAC remplacées ; rejouée par les tests de `crates/moli-unifi/src/lib.rs`.
- Certificat épinglé au premier contact, avant toute clé.
- Images à la demande, mouvement par le websocket d'événements, et vidéo en direct pour le pont
  HomeKit : à la demande, le pilote demande à la console de publier un flux RTSPS (`POST
  /cameras/{id}/rtsps-stream`, qualités haute/moyenne/basse) et le lit en RTSP sur TLS, port 7441, dont le
  certificat est épinglé à part (secret `rtsps_cert_sha256`, au premier contact). Aucun réglage de caméra
  n'est modifié ; la console garde les flux publiés. Non essayé sur une vraie console.
- Reste : le format exact des événements du websocket n'a pas été relevé sur une vraie console
  (le pilote est tolérant et journalise chaque message en `debug`) ; pas de détection
  d'objets, ces caméras ne la gèrent pas ; pas de sonnette ; pas de découverte.
