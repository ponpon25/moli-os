# Accessoire HomeKit

Moli s'appaire à un accessoire HomeKit comme le ferait l'app Maison, puis reçoit ses événements
sur le réseau local, sans cloud : l'appui d'une sonnette arrive en une fraction de seconde.

- Un appareil par accessoire (le premier `aid`), nommé d'après son service d'information.
- Points (lecture seule) : `ring` (bouton d'un service Sonnette, impulsion), `button` (autre
  interrupteur programmable, impulsion), `motion` (détecteur de mouvement).
- Une caméra (service de gestion du flux) : direct H.264 et son Opus 16 kHz en SRTP, image fixe
  (`/resource`), et la voix de la personne vers son haut-parleur ; le pont HomeKit de Moli la
  remontre à l'app Maison, en **sonnette vidéo** quand l'appareil a un point `ring`.
- Session chiffrée HAP (pair-verify), abonnement aux événements, vérification toutes les 30 s ;
  reconnexion avec attente croissante (2 s à 1 min).
- Secrets : `setup_code` (donné une fois par la personne, oublié après l'appairage), `pairing`
  (écrit par le pilote : identité de Moli et clé de l'accessoire).
- Un accessoire HomeKit n'a qu'un propriétaire : il faut le retirer de l'app Maison avant.
- Pas encore : accessoires derrière un pont (plusieurs `aid`), écriture, découverte mDNS
  (l'adresse est donnée dans la config).
