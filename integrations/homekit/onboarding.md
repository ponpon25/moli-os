# Onboarding : accessoire HomeKit

1. Retirer l'accessoire de l'app Maison (fiche de l'accessoire › Réglages › Supprimer
   l'accessoire). Pas de réinitialisation : il reste sur le Wi-Fi et dans l'appli de sa marque.
   Sur le réseau, son service `_hap._tcp` annonce alors `sf=1` (libre).
2. Trouver son adresse et son port (`dns-sd -B _hap._tcp`, puis `dns-sd -L <nom> _hap._tcp`) ;
   lui donner une adresse fixe dans le routeur.
3. Ranger son code HomeKit à 8 chiffres (étiquette sur l'appareil ou sa notice), **sans
   l'afficher** : `… | docker exec -i moli-os /moli-os secrets set <instance> setup_code`.
4. `[[driver]] kind = "homekit"`, `[driver.options] address = "192.168.1.x:port"` ; redémarrer.
   Le pilote s'appaire une fois, garde l'appairage dans le coffre et oublie le code. Un code faux
   n'est jamais réessayé seul (l'accessoire se verrouille après quelques essais).
5. Tester : un appui sur la sonnette passe le point `ring` à oui puis non (journal :
   « homekit press »).
