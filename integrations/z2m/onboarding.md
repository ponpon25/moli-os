# Onboarding : Zigbee2MQTT

1. Demander : où tourne le broker MQTT (hôte, port, 1883 par défaut), le `base_topic` de
   Zigbee2MQTT (`zigbee2mqtt` par défaut), et s'il exige un utilisateur.
2. Mot de passe éventuel : dans une variable d'environnement dont seul le **nom** va dans
   `password_env` ; jamais dans `moli.toml`.
3. Appairer les appareils dans Zigbee2MQTT lui-même (Moli ne fait pas l'appairage Zigbee).
   Avec `frontend = "http://<hôte>:8080"`, Moli met un lien vers son interface sur la page
   Équipements (groupe z2m) et dans le détail du pilote (Système).
4. Déclarer `kind = "z2m"` ; `moli-os check-config`, redémarrer ; tous les appareils de
   `bridge/devices` apparaissent (coordinateur et appareils désactivés exclus).
5. Tester une commande sur une cible neutre, humain présent, jamais dans une chambre.
