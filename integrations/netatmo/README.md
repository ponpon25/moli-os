# Sonnette vidéo Netatmo

- Origine : rien de repris. API Netatmo Connect (Security), `api.netatmo.com` : `POST /oauth2/token`
  (échange du jeton de rafraîchissement), `GET /api/homesdata` (modules de type `NDB`),
  `GET /api/getevents`, `GET /api/homestatus`.
- `fixtures/homesdata.json`, `fixtures/events.json` : réponses **fictives** reconstituées de la doc et de
  rapports de la communauté, rejouées par les tests de `crates/moli-netatmo`. Pas encore comparées à une
  vraie réponse de la sonnette.
- Lecture seule : `ring` (appui sur le bouton), `visitor` (personne vue), `last_event` (type du dernier
  événement). Temps réel par le **webhook** de Netatmo (`POST /api/addwebhook`, adresse
  `{public_url}/api/netatmo/{webhook_token}`) : la poussée réveille le pilote, qui relit `getevents`. En
  filet de sécurité, ou sans adresse publique, les événements sont lus à intervalles réguliers (60 s avec
  webhook, 10 s sans, 8 s au plus bas : Netatmo limite à 500 appels par heure et par utilisateur).
- Le jeton de rafraîchissement ne sert qu'une fois : chaque échange en rend un nouveau, rangé au coffre
  avant tout autre usage.
- Reste : confirmer les noms d'événements sur une vraie sonnette (le pilote reconnaît `incoming_call` et
  garde le type brut dans `last_event`) ; le webhook n'a pas été essayé avec la vraie API (portée exigée par `addwebhook` à confirmer) ; vidéo en direct.
