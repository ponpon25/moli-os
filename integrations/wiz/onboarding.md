# Onboarding : WiZ

1. Demander : l'adresse IP de la lampe (à réserver sur la box), et vérifier dans l'app WiZ que la
   communication locale est permise (Réglages → Sécurité). Aucun secret.
2. Déclarer `kind = "wiz"`, `host`, et `name` (la lampe ne garde pas son nom sur le réseau) ;
   `moli-os check-config`, redémarrer.
3. Déjà dans Apple Maison en direct : la masquer du pont HomeKit de Moli, sinon elle y apparaît
   deux fois.
4. Tester, humain présent, hors chambre : éteindre puis rallumer, changer la luminosité.
