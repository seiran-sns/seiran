use seiran_common::{get_db_pool, run_migrations};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Connecting to database...");
    let pool = get_db_pool().await?;
    println!("Database connected successfully.");
    
    println!("Running migrations...");
    run_migrations(&pool).await?;
    println!("Migrations applied successfully!");
    
    Ok(())
}
