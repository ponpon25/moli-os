# Onboarding : Olvid

Olvid est une messagerie chiffrée de bout en bout, sans numéro de téléphone. Moli y a son propre
« bot » (une identité Olvid) ; les personnes l'ajoutent comme un contact.

1. Avoir un daemon Olvid (`olvid/bot-daemon`) qui tourne et que Moli atteint en `host:port`
   (publier son port 50051 sur `127.0.0.1` du serveur, ou l'adresse du conteneur). Sa clé
   d'administration est la variable `OLVID_ADMIN_CLIENT_KEY` du conteneur.
2. Ranger cette clé dans le coffre de Moli, **sans la taper ni l'afficher** :
   `… | docker exec -i moli-os /moli-os secrets set <instance> admin_key`.
3. `[[driver]] kind = "olvid"`, `[driver.options] daemon = "127.0.0.1:50051"` (et `bot_name`,
   « Moli » par défaut) ; redémarrer Moli. Il crée son identité et sa clé client (rangées dans le
   coffre) ; la clé d'administration ne sert plus.
4. Appairer chaque personne (le code du foyer est demandé) : Réglages › Intégrations › Olvid ›
   « Appairer un téléphone » ; ouvrir le lien dans Olvid ; comparer les deux codes à quatre
   chiffres. La fenêtre d'appairage dure 15 minutes et se ferme dès qu'une personne est reliée.
5. Tester : une commande `notify` avec un texte court.
