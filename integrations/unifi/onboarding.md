# Onboarding : UniFi Protect

1. Demander l'adresse IP de la console (Cloud Key, UDM, NVR).
2. Dans Protect : barre de gauche, icône « Intégrations » (prise) → « Create New API Key »,
   nom « Moli ». La clé n'est montrée qu'une fois.
3. Il faut une clé maître (`data/master.key`) : le pilote range lui-même l'empreinte du
   certificat dans le coffre.
4. Déclarer `kind = "unifi"`, `host = "<ip>"` ; `moli-os check-config`, redémarrer. Le tableau
   de bord dit alors que la clé manque.
5. La clé passe par un tube, jamais au clavier ni à l'écran :
   `read -rs V; printf %s "$V" | docker exec -i moli-os /moli-os secrets set <instance> api_key`,
   puis redémarrer.
6. Tester : les caméras apparaissent (Caméras), une image s'affiche, passer devant une caméra
   allume son point « Mouvement ».
