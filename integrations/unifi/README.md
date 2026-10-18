# UniFi Protect

- Origine : rien de noté (API d'intégration de la console : `/proxy/protect/integration/v1`,
  clé dans l'en-tête `X-API-KEY`).
- `fixtures/cameras.json` : la liste de caméras d'une vraie Cloud Key Gen2 Plus (Protect 7.3.70),
  adresses MAC remplacées ; rejouée par les tests de `crates/moli-unifi/src/lib.rs`.
- Certificat épinglé au premier contact, avant toute clé.
- Lecture seule : images à la demande, mouvement par le websocket d'événements.
- Reste : le format exact des événements du websocket n'a pas été relevé sur une vraie console
  (le pilote est tolérant et journalise chaque message en `debug`) ; pas de détection
  d'objets, ces caméras ne la gèrent pas ; pas de sonnette ; pas de découverte.
