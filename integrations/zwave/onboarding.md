# Onboarding : Z-Wave

1. Brancher la clé Z-Wave ; la passer au conteneur de Z-Wave JS UI par son nom stable
   (`/dev/serial/by-id/…`), groupe `dialout`.
2. Z-Wave JS UI (`zwavejs/zwave-js-ui`, réseau hôte) : Paramètres › Z-Wave : le port série,
   générer les clés de sécurité (S0, S2) ; Paramètres › Home Assistant : activer le
   **WS Server**, hôte `127.0.0.1`, port 3000 ; Paramètres › Général : activer
   l'authentification de l'interface.
3. Déclarer `kind = "zwave"` (options `frontend = "http://<hôte>:8091"`), `moli-os
   check-config`, redémarrer.
4. Inclure les modules depuis Z-Wave JS UI (« Appairage et réglages » dans Équipements) :
   ils apparaissent dans Moli après leur interview.
