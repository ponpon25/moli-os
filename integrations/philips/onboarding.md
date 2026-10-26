# Onboarding : télé Philips

1. Demander : l'adresse IP de la télé (fixe : elle sert d'identité) et son adresse MAC (pour
   l'allumer par Wake-on-LAN).
2. Il faut une clé maître (le pilote range lui-même les identifiants d'appairage).
3. Déclarer `kind = "philips"`, `host`, `mac` ; `moli-os check-config`, redémarrer.
   Appairer, humain devant la télé allumée : écrire `true` dans le point `pairing`, puis
   le code affiché dans `pin` (dans la minute). Le pilote passe seul en marche normale.
4. Tester, humain présent : baisser puis remonter le volume ; en veille profonde, la télé est
   hors ligne (`power = false`), c'est normal.
5. Applis et « ce qui se joue » : Cast marche seul. Pour ouvrir des applis, reprendre
   l'appairage Android TV Remote de HA s'il existe :
   `cat …/.storage/androidtv_remote_cert.pem | docker exec -i moli-os /moli-os secrets set <instance> atv_cert`,
   idem `androidtv_remote_key.pem` → `atv_key`, puis redémarrer.
