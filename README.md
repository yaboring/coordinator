
## Coordinator for "self-governing" entities

## requirements
    PHP, a MySQL client, docker and kubectl (and a kubernetes cluster, with kubectl pointing to it)
    I'll try to run the whole thing inside the cluster so you don't need to install dirty PHP, promise

## create namespaces to separate static and dynamic infra
    kubectl apply -f ./kubernetes/namespaces.yaml

## create our source of truth
    kubectl apply -f ./kubernetes/database.yaml

## put some content in the database (localhost:30006)
    CREATE TABLE yaboring.`entity_types` (
        `id` bigint unsigned NOT NULL AUTO_INCREMENT,
        `title` varchar(100) COLLATE utf8mb4_general_ci NOT NULL,
        `description` varchar(512) COLLATE utf8mb4_general_ci DEFAULT NULL,
        PRIMARY KEY (`id`)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

    INSERT INTO yaboring.entity_types (title, description) VALUES
	 ('factory','Produces goods'),
	 ('warehouse','Stores goods'),
	 ('truck','Transports goods'),
	 ('client','Receives goods');

    CREATE TABLE yaboring.entities (
        id BIGINT UNSIGNED auto_increment NOT NULL,
        `type` BIGINT UNSIGNED NOT NULL,
        status ENUM('inactive','active') DEFAULT 'inactive' NOT NULL,
        self_governed BOOL DEFAULT 0 NOT NULL,
        CONSTRAINT entities_pk PRIMARY KEY (id),
        CONSTRAINT entities_entity_types_FK FOREIGN KEY (`type`) REFERENCES yaboring.entity_types(id)
    )
    ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

    INSERT INTO yaboring.entities (`type`, status, self_governed) VALUES
	 ((SELECT id from entity_types where title = 'factory'),'active',1),
	 ((SELECT id from entity_types where title = 'warehouse'),'active',1),
	 ((SELECT id from entity_types where title = 'truck'),'active',1),
	 ((SELECT id from entity_types where title = 'client'),'active',1);

## prepare docker image for entity pods
    docker build ./entity/. -t yaboring-entity:v0.1

## generate NPCs from database content and create them in our cluster, in their own namespace
    php ./kubernetes/genConfig.php
    kubectl apply -f ./kubernetes/entities --prune --all --prune-allowlist=core/v1/Pod --prune-allowlist=core/v1/Service

## reload pod images - there's a better way for sure
    kubectl delete namespace yaboring-entities
    kubectl apply -f ./kubernetes/namespaces.yaml
    kubectl apply -f ./kubernetes/entities --prune --all --prune-allowlist=core/v1/Pod --prune-allowlist=core/v1/Service

    kubectl delete namespace yaboring-entities && kubectl apply -f ./kubernetes/namespaces.yaml && kubectl apply -f ./kubernetes/entities --prune --all --prune-allowlist=core/v1/Pod --prune-allowlist=core/v1/Service

## test entity image
    docker run --name dev-entity -p 8080 yaboring-entity:v0.1


## TODO
    - if no active entities, no YAML comes out, kubectl does no changes

