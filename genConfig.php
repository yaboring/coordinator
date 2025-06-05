<?php

$dBHost = 'localhost:30006';
$dBUsername = 'root';
$dBPassword = 'password';
$yaboringDBName = 'yaboring';

$db = new mysqli($dBHost, $dBUsername, $dBPassword, $yaboringDBName);

$db->fetch = function($query) use ($db) {
    $result = $db->query($query);
    $data = $result->fetch_all(MYSQLI_ASSOC);
    $result->free();
    return $data;
};


$entities = ($db->fetch)("
    SELECT e.id
    FROM entities e
    WHERE e.status = 'active'
    AND e.self_governed = true
");

$localDir = __DIR__;

$files = glob("{$localDir}/entities/*.yaml");
foreach($files as $file){
    if(is_file($file)) {
      unlink($file);
    }
}

foreach($entities as $entity){
        
    $entityID = $entity['id'];
    $entityPort = 30080 + $entityID;

    file_put_contents("{$localDir}/entities/entity-{$entityID}.yaml",
"
apiVersion: v1
kind: Pod
metadata:
  namespace: yaboring-entities
  name: entity-{$entityID}
  labels:
    app.kubernetes.io/instance: entity-{$entityID}
spec:
  containers:
  - name: entity-{$entityID}
    image: yaboring-entity:v0.1
    env:
    - name: YABORING_ENTITY_ID
      value: \"{$entityID}\"
---
apiVersion: v1
kind: Service
metadata:
  namespace: yaboring-entities
  name: entity-service-{$entityID}
  labels:
    app.kubernetes.io/instance: entity-service-{$entityID}
spec:
  type: NodePort
  selector:
    app.kubernetes.io/instance: entity-{$entityID}
  ports:
    - port: 8080
      targetPort: 8080
      nodePort: {$entityPort}
"
    );

}
