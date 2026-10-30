# Onboarding : sonnette vidéo Netatmo

1. Alimenter la sonnette : elle est filaire, en basse tension (voir la notice : 8 à 24 V). Sans carillon
   existant, un petit transformateur de sonnette ou une alimentation basse tension suffit ; le carillon
   mécanique est facultatif. L'ajouter à l'appli Netatmo Home + Security (compte du foyer).
2. Sur https://dev.netatmo.com, créer une application, puis dans son générateur de jetons choisir la portée
   `read_doorbell` et générer un jeton : on obtient un `client_id`, un `client_secret` et un
   `refresh_token`.
3. Il faut une clé maître (`data/master.key`). Déclarer `kind = "netatmo"` (options facultatives :
   `public_url`, le nom public de Moli en `https://…` sans chemin, pour le temps réel ; `poll_secs`, 10 par
   défaut, 8 au plus bas) ; `moli-os check-config`, redémarrer. Le tableau de bord dit
   que les identifiants manquent.
4. Les trois valeurs passent chacune par un tube, jamais au clavier ni à l'écran :
   `read -rs V; printf %s "$V" | docker exec -i moli-os /moli-os secrets set <instance> client_id`
   (puis `client_secret`, puis `refresh_token`), puis redémarrer. **Un seul échange par démarrage** ; un refus
   de Netatmo arrête le pilote (`login_locked`) : refaire un jeton, `secrets set … refresh_token`,
   `secrets forget <instance> login_locked`, redémarrer.
5. **Temps réel (webhook)** : avec `public_url` (le nom déjà ouvert à Internet pour Alexa convient : il
   répond alors aussi à `POST /api/netatmo/<jeton>`, rien d'autre), ranger un jeton tiré au hasard,
   jamais tapé ni affiché :
   `head -c 30 /dev/urandom | base64 | tr -d '/+=\n' | docker exec -i moli-os /moli-os secrets set <instance> webhook_token`,
   puis redémarrer. Le pilote donne l'adresse à Netatmo (`/api/addwebhook`), et la redonne chaque heure. Une
   poussée de Netatmo ne fait que **réveiller** le pilote, qui relit alors les événements par l'API : un
   appel forgé ne peut rien sonner. Sans webhook (ou s'il est refusé), l'interrogation seule continue, toutes
   les 10 s ; avec, une fois par minute en filet de sécurité.
6. La sonnette apparaît seule. Appuyer sur le bouton : `ring` s'allume au plus tard à la période
   d'interrogation. Un carillon sur une enceinte et une notification sont des automatisations sur ce point.
